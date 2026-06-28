//! Comprehensive performance harness for extreme-load and adversarial traffic.
//!
//! Run manually:
//! `cargo test --release --features integration --test performance_harness -- --ignored --nocapture`
#![cfg(feature = "integration")]

use serde::Serialize;
use serde_json::{Value, json};
use serval::api;
use serval::auth::{AuthConfig, AuthService};
use serval::cache::DeliveryCache;
use serval::crypto::IdSigner;
use serval::db::{self, Repository};
use serval::delivery;
use serval::state::{ControlState, DeliveryState};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

const TEST_ID_SECRET: &str = "performance-suite-id-signing-secret-please";

#[derive(Clone)]
struct Harness {
    control_base: String,
    data_base: String,
    client: reqwest::Client,
    _pg: Arc<ContainerAsync<Postgres>>,
}

impl Harness {
    async fn start() -> Self {
        let pg = Postgres::default()
            .start()
            .await
            .expect("failed to start postgres container");
        let host = pg.get_host().await.expect("container host");
        let port = pg
            .get_host_port_ipv4(5432)
            .await
            .expect("container port mapping");
        let database_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

        let pool = db::connect(&database_url, 16)
            .await
            .expect("failed to connect and apply schema");
        let repo = Repository::new(pool);

        let cache = DeliveryCache::new(64 * 1024 * 1024);
        let signer = IdSigner::new(TEST_ID_SECRET);
        let auth = Arc::new(
            AuthService::new(AuthConfig::None)
                .await
                .expect("auth service"),
        );

        let control_state = ControlState {
            repo: repo.clone(),
            cache: cache.clone(),
            auth,
            signer: signer.clone(),
            data_plane_url: None,
        };
        let data_state = DeliveryState {
            repo,
            cache,
            signer,
        };

        let control_base = serve(api::router(control_state)).await;
        let data_base = serve(delivery::router(data_state)).await;

        Self {
            control_base,
            data_base,
            client: reqwest::Client::new(),
            _pg: Arc::new(pg),
        }
    }

    async fn create(&self, body: Value) -> Value {
        let resp = self
            .client
            .post(format!("{}/api/snippets", self.control_base))
            .json(&body)
            .send()
            .await
            .expect("create request");
        assert!(
            resp.status().is_success(),
            "create failed: {}",
            resp.status()
        );
        resp.json().await.expect("create json")
    }

    async fn patch_content(&self, id: &str, content: &str) {
        let resp = self
            .client
            .patch(format!("{}/api/snippets/{id}", self.control_base))
            .json(&json!({ "content": content }))
            .send()
            .await
            .expect("patch request");
        assert!(
            resp.status().is_success(),
            "patch failed: {}",
            resp.status()
        );
    }

    async fn get_status(&self, path: &str) -> reqwest::StatusCode {
        self.client
            .get(format!("{}/{}", self.data_base, path))
            .send()
            .await
            .expect("data-plane request")
            .status()
    }
}

async fn serve(router: axum::Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    format!("http://{addr}")
}

#[derive(Default)]
struct AtomicStats {
    total: AtomicU64,
    success: AtomicU64,
    status_404: AtomicU64,
    client_error: AtomicU64,
    server_error: AtomicU64,
    latency_micros: Mutex<Vec<u64>>,
}

#[derive(Clone, Copy, Serialize)]
struct Snapshot {
    total: u64,
    success: u64,
    status_404: u64,
    client_error: u64,
    server_error: u64,
    error_rate: f64,
    throughput_rps: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
}

impl AtomicStats {
    async fn record(&self, status: reqwest::StatusCode, latency: Duration) {
        self.total.fetch_add(1, Ordering::Relaxed);
        if status.is_success() {
            self.success.fetch_add(1, Ordering::Relaxed);
        } else if status.as_u16() == 404 {
            self.status_404.fetch_add(1, Ordering::Relaxed);
        } else if status.is_client_error() {
            self.client_error.fetch_add(1, Ordering::Relaxed);
        } else if status.is_server_error() {
            self.server_error.fetch_add(1, Ordering::Relaxed);
        }

        let micros_u128 = latency.as_micros();
        let micros = u64::try_from(micros_u128).unwrap_or(u64::MAX);
        self.latency_micros.lock().await.push(micros);
    }

    async fn snapshot(&self, elapsed: Duration) -> Snapshot {
        let mut samples = self.latency_micros.lock().await.clone();
        samples.sort_unstable();

        let p = |q: f64| -> f64 {
            if samples.is_empty() {
                return 0.0;
            }
            let idx = ((samples.len() - 1) as f64 * q).round() as usize;
            samples[idx] as f64 / 1_000.0
        };

        let total = self.total.load(Ordering::Relaxed);
        let success = self.success.load(Ordering::Relaxed);
        let status_404 = self.status_404.load(Ordering::Relaxed);
        let client_error = self.client_error.load(Ordering::Relaxed);
        let server_error = self.server_error.load(Ordering::Relaxed);
        let non_success = total.saturating_sub(success);
        let error_rate = if total == 0 {
            0.0
        } else {
            non_success as f64 / total as f64
        };

        Snapshot {
            total,
            success,
            status_404,
            client_error,
            server_error,
            error_rate,
            throughput_rps: if elapsed.is_zero() {
                0.0
            } else {
                total as f64 / elapsed.as_secs_f64()
            },
            p50_ms: p(0.50),
            p95_ms: p(0.95),
            p99_ms: p(0.99),
        }
    }
}

#[derive(Serialize)]
struct Report {
    peak: Snapshot,
    adversarial_legit: Snapshot,
    adversarial_forged_total: u64,
    adversarial_forged_404: u64,
    adversarial_forged_rejection_rate: f64,
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

async fn run_peak_profile(h: &Harness, id: &str) -> Snapshot {
    let concurrency = env_u64("PERF_PEAK_CONCURRENCY", 96) as usize;
    let duration_secs = env_u64("PERF_PEAK_DURATION_SECS", 15);
    let duration = Duration::from_secs(duration_secs);

    let stats = Arc::new(AtomicStats::default());
    let start = Instant::now();
    let mut tasks = Vec::with_capacity(concurrency);

    for worker in 0..concurrency {
        let h = h.clone();
        let stats = Arc::clone(&stats);
        let id = id.to_owned();
        tasks.push(tokio::spawn(async move {
            let mut n = worker as u64;
            while start.elapsed() < duration {
                let path = if n % 10 == 0 {
                    format!("{id}?port=8080&region=us-east&cache_buster={n}")
                } else {
                    format!("{id}?port=8080&region=us-east")
                };
                let t0 = Instant::now();
                let status = h.get_status(&path).await;
                stats.record(status, t0.elapsed()).await;
                n = n.wrapping_add(1);
            }
        }));
    }

    for task in tasks {
        task.await.expect("peak worker panicked");
    }

    stats.snapshot(start.elapsed()).await
}

async fn run_adversarial_profile(h: &Harness, id: &str) -> (Snapshot, u64, u64) {
    let legit_concurrency = env_u64("PERF_ADVERSARIAL_LEGIT_CONCURRENCY", 36) as usize;
    let forged_concurrency = env_u64("PERF_ADVERSARIAL_FORGED_CONCURRENCY", 72) as usize;
    let writer_interval_ms = env_u64("PERF_ADVERSARIAL_WRITER_INTERVAL_MS", 80);
    let duration_secs = env_u64("PERF_ADVERSARIAL_DURATION_SECS", 15);
    let duration = Duration::from_secs(duration_secs);

    let attack_stop = Arc::new(AtomicBool::new(false));
    let forged_total = Arc::new(AtomicU64::new(0));
    let forged_404 = Arc::new(AtomicU64::new(0));

    let mut forged_tasks = Vec::with_capacity(forged_concurrency);
    for worker in 0..forged_concurrency {
        let h = h.clone();
        let stop = Arc::clone(&attack_stop);
        let total = Arc::clone(&forged_total);
        let rejected = Arc::clone(&forged_404);
        forged_tasks.push(tokio::spawn(async move {
            let mut n = worker as u64;
            while !stop.load(Ordering::Relaxed) {
                let forged = format!("forged-id-{worker}-{n}");
                let status = h.get_status(&forged).await;
                total.fetch_add(1, Ordering::Relaxed);
                if status.as_u16() == 404 {
                    rejected.fetch_add(1, Ordering::Relaxed);
                }
                n = n.wrapping_add(1);
            }
        }));
    }

    let writer_stop = Arc::new(AtomicBool::new(false));
    let writer_handle = {
        let h = h.clone();
        let id = id.to_owned();
        let stop = Arc::clone(&writer_stop);
        tokio::spawn(async move {
            let mut version = 0_u64;
            while !stop.load(Ordering::Relaxed) {
                let body = format!("payload-version-{version}");
                h.patch_content(&id, &body).await;
                version = version.wrapping_add(1);
                tokio::time::sleep(Duration::from_millis(writer_interval_ms)).await;
            }
        })
    };

    let legit_stats = Arc::new(AtomicStats::default());
    let start = Instant::now();
    let mut legit_tasks = Vec::with_capacity(legit_concurrency);
    for worker in 0..legit_concurrency {
        let h = h.clone();
        let stats = Arc::clone(&legit_stats);
        let id = id.to_owned();
        legit_tasks.push(tokio::spawn(async move {
            let mut n = worker as u64;
            while start.elapsed() < duration {
                let path = if n % 2 == 0 {
                    format!("{id}?port=3000&tenant=prod")
                } else {
                    format!("{id}?port=3000&tenant=prod&noise={n}")
                };
                let t0 = Instant::now();
                let status = h.get_status(&path).await;
                stats.record(status, t0.elapsed()).await;
                n = n.wrapping_add(1);
            }
        }));
    }

    for task in legit_tasks {
        task.await.expect("legit worker panicked");
    }

    writer_stop.store(true, Ordering::Relaxed);
    writer_handle.await.expect("writer panicked");

    attack_stop.store(true, Ordering::Relaxed);
    for task in forged_tasks {
        task.await.expect("forged worker panicked");
    }

    let legit = legit_stats.snapshot(start.elapsed()).await;
    (
        legit,
        forged_total.load(Ordering::Relaxed),
        forged_404.load(Ordering::Relaxed),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "heavy performance harness; run in dedicated CI workflow"]
async fn comprehensive_performance_harness() {
    let harness = Harness::start().await;
    let created = harness
        .create(json!({ "content": "Hello {{tenant}} on {{port}}" }))
        .await;
    let id = created["id"].as_str().expect("snippet id").to_owned();

    let peak = run_peak_profile(&harness, &id).await;
    let (adversarial_legit, forged_total, forged_404) =
        run_adversarial_profile(&harness, &id).await;

    let forged_rejection_rate = if forged_total == 0 {
        0.0
    } else {
        forged_404 as f64 / forged_total as f64
    };

    let report = Report {
        peak,
        adversarial_legit,
        adversarial_forged_total: forged_total,
        adversarial_forged_404: forged_404,
        adversarial_forged_rejection_rate: forged_rejection_rate,
    };

    let report_json = serde_json::to_string_pretty(&report).expect("serialize report");
    println!("{report_json}");

    let report_path = std::env::var("PERF_RESULTS_PATH")
        .unwrap_or_else(|_| "target/performance-harness-report.json".to_owned());
    if let Some(parent) = std::path::Path::new(&report_path).parent() {
        std::fs::create_dir_all(parent).expect("create report directory");
    }
    std::fs::write(&report_path, report_json).expect("write report file");

    let peak_max_p95_ms = env_u64("PERF_ASSERT_PEAK_P95_MS", 350) as f64;
    let peak_min_rps = env_u64("PERF_ASSERT_PEAK_MIN_RPS", 350) as f64;
    let peak_max_error_rate = env_u64("PERF_ASSERT_PEAK_MAX_ERROR_PCT", 2) as f64 / 100.0;

    let adversarial_max_p95_ms = env_u64("PERF_ASSERT_ADVERSARIAL_P95_MS", 450) as f64;
    let adversarial_min_rps = env_u64("PERF_ASSERT_ADVERSARIAL_MIN_RPS", 120) as f64;
    let adversarial_max_error_rate =
        env_u64("PERF_ASSERT_ADVERSARIAL_MAX_ERROR_PCT", 5) as f64 / 100.0;
    let min_forged_rejection_pct = env_u64("PERF_ASSERT_FORGED_REJECTION_PCT", 99) as f64 / 100.0;

    assert!(
        report.peak.p95_ms <= peak_max_p95_ms,
        "peak p95 too high: {:.2}ms > {:.2}ms",
        report.peak.p95_ms,
        peak_max_p95_ms
    );
    assert!(
        report.peak.throughput_rps >= peak_min_rps,
        "peak throughput too low: {:.2} rps < {:.2} rps",
        report.peak.throughput_rps,
        peak_min_rps
    );
    assert!(
        report.peak.error_rate <= peak_max_error_rate,
        "peak error rate too high: {:.4} > {:.4}",
        report.peak.error_rate,
        peak_max_error_rate
    );

    assert!(
        report.adversarial_legit.p95_ms <= adversarial_max_p95_ms,
        "adversarial legit p95 too high: {:.2}ms > {:.2}ms",
        report.adversarial_legit.p95_ms,
        adversarial_max_p95_ms
    );
    assert!(
        report.adversarial_legit.throughput_rps >= adversarial_min_rps,
        "adversarial legit throughput too low: {:.2} rps < {:.2} rps",
        report.adversarial_legit.throughput_rps,
        adversarial_min_rps
    );
    assert!(
        report.adversarial_legit.error_rate <= adversarial_max_error_rate,
        "adversarial legit error rate too high: {:.4} > {:.4}",
        report.adversarial_legit.error_rate,
        adversarial_max_error_rate
    );
    assert!(
        report.adversarial_forged_rejection_rate >= min_forged_rejection_pct,
        "forged rejection rate too low: {:.4} < {:.4}",
        report.adversarial_forged_rejection_rate,
        min_forged_rejection_pct
    );
}
