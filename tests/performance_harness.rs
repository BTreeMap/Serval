//! Comprehensive performance harness for adaptive extreme-load and adversarial
//! Data Plane traffic.
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

    async fn patch_content(&self, id: &str, content: &str) -> reqwest::Result<reqwest::StatusCode> {
        let resp = self
            .client
            .patch(format!("{}/api/snippets/{id}", self.control_base))
            .json(&json!({ "content": content }))
            .send()
            .await?;
        Ok(resp.status())
    }

    async fn get_status(&self, path: &str) -> reqwest::Result<reqwest::StatusCode> {
        let resp = self
            .client
            .get(format!("{}/{}", self.data_base, path))
            .send()
            .await?;
        Ok(resp.status())
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

struct AtomicStats {
    total: AtomicU64,
    success: AtomicU64,
    status_404: AtomicU64,
    client_error: AtomicU64,
    server_error: AtomicU64,
    transport_error: AtomicU64,
    sample_clock: AtomicU64,
    sample_stride: u64,
    latency_micros: Mutex<Vec<u32>>,
}

#[derive(Clone, Copy, Serialize)]
struct Snapshot {
    total: u64,
    success: u64,
    status_404: u64,
    client_error: u64,
    server_error: u64,
    transport_error: u64,
    error_rate: f64,
    throughput_rps: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
}

impl AtomicStats {
    fn new(sample_stride: u64, sample_capacity_hint: usize) -> Self {
        Self {
            total: AtomicU64::new(0),
            success: AtomicU64::new(0),
            status_404: AtomicU64::new(0),
            client_error: AtomicU64::new(0),
            server_error: AtomicU64::new(0),
            transport_error: AtomicU64::new(0),
            sample_clock: AtomicU64::new(0),
            sample_stride: sample_stride.max(1),
            latency_micros: Mutex::new(Vec::with_capacity(sample_capacity_hint)),
        }
    }

    async fn record_status(&self, status: reqwest::StatusCode, latency: Duration) {
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
        self.record_latency_sample(latency).await;
    }

    async fn record_transport_error(&self, latency: Duration) {
        self.total.fetch_add(1, Ordering::Relaxed);
        self.transport_error.fetch_add(1, Ordering::Relaxed);
        self.record_latency_sample(latency).await;
    }

    async fn record_latency_sample(&self, latency: Duration) {
        let sample_idx = self.sample_clock.fetch_add(1, Ordering::Relaxed);
        if sample_idx % self.sample_stride != 0 {
            return;
        }
        let micros_u128 = latency.as_micros();
        let micros = u32::try_from(micros_u128).unwrap_or(u32::MAX);
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
        let transport_error = self.transport_error.load(Ordering::Relaxed);
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
            transport_error,
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

#[derive(Clone, Serialize)]
struct PeakStageReport {
    concurrency: u64,
    duration_secs: u64,
    healthy: bool,
    snapshot: Snapshot,
}

#[derive(Clone, Serialize)]
struct AdversarialStageReport {
    forged_concurrency: u64,
    legit_concurrency: u64,
    duration_secs: u64,
    healthy: bool,
    legit: Snapshot,
    forged_total: u64,
    forged_404: u64,
    forged_transport_error: u64,
    forged_rejection_rate: f64,
    control_write_total: u64,
    control_write_success: u64,
    control_write_error: u64,
    control_write_success_rate: f64,
}

#[derive(Serialize)]
struct Report {
    peak_stages: Vec<PeakStageReport>,
    peak_best: PeakStageReport,
    peak_max_tested_concurrency: u64,
    adversarial_stages: Vec<AdversarialStageReport>,
    adversarial_best: AdversarialStageReport,
    adversarial_max_tested_forged_concurrency: u64,
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn next_concurrency(current: u64, max: u64, multiplier_pct: u64) -> u64 {
    if current >= max {
        return max;
    }
    let mult = multiplier_pct.max(101);
    let grown = current
        .saturating_mul(mult)
        .saturating_div(100)
        .max(current + 1);
    grown.min(max)
}

fn stage_is_healthy(snapshot: Snapshot, max_error_rate: f64, max_p95_ms: f64) -> bool {
    snapshot.error_rate <= max_error_rate && snapshot.p95_ms <= max_p95_ms
}

fn choose_best_peak_stage(stages: &[PeakStageReport]) -> PeakStageReport {
    stages
        .iter()
        .filter(|stage| stage.healthy)
        .max_by(|a, b| {
            a.snapshot
                .throughput_rps
                .total_cmp(&b.snapshot.throughput_rps)
        })
        .or_else(|| {
            stages.iter().max_by(|a, b| {
                a.snapshot
                    .throughput_rps
                    .total_cmp(&b.snapshot.throughput_rps)
            })
        })
        .expect("at least one peak stage")
        .clone()
}

fn choose_best_adversarial_stage(stages: &[AdversarialStageReport]) -> AdversarialStageReport {
    stages
        .iter()
        .filter(|stage| stage.healthy)
        .max_by(|a, b| a.forged_concurrency.cmp(&b.forged_concurrency))
        .or_else(|| {
            stages
                .iter()
                .max_by(|a, b| a.forged_concurrency.cmp(&b.forged_concurrency))
        })
        .expect("at least one adversarial stage")
        .clone()
}

async fn run_data_plane_stage(
    h: &Harness,
    id: &str,
    concurrency: usize,
    duration: Duration,
) -> Snapshot {
    let sample_stride = env_u64("PERF_LATENCY_SAMPLE_STRIDE", 16);
    let sample_capacity_hint = env_u64("PERF_LATENCY_SAMPLE_CAPACITY", 200_000) as usize;
    let stats = Arc::new(AtomicStats::new(sample_stride, sample_capacity_hint));

    let start = Instant::now();
    let mut tasks = Vec::with_capacity(concurrency);

    for worker in 0..concurrency {
        let h = h.clone();
        let stats = Arc::clone(&stats);
        let id = id.to_owned();
        tasks.push(tokio::spawn(async move {
            let mut n = worker as u64;
            while start.elapsed() < duration {
                let path = if n % 8 == 0 {
                    format!("{id}?port=8080&region=us-east&cache_buster={n}")
                } else {
                    format!("{id}?port=8080&region=us-east")
                };
                let t0 = Instant::now();
                match h.get_status(&path).await {
                    Ok(status) => stats.record_status(status, t0.elapsed()).await,
                    Err(_) => stats.record_transport_error(t0.elapsed()).await,
                }
                n = n.wrapping_add(1);
            }
        }));
    }

    for task in tasks {
        task.await.expect("data-plane stage worker panicked");
    }

    stats.snapshot(start.elapsed()).await
}

async fn run_peak_profile(h: &Harness, id: &str) -> (Vec<PeakStageReport>, PeakStageReport, u64) {
    let mut current = env_u64("PERF_PEAK_INITIAL_CONCURRENCY", 256);
    let max_concurrency = env_u64("PERF_PEAK_MAX_CONCURRENCY", 16_384);
    let stage_duration_secs = env_u64("PERF_PEAK_STAGE_DURATION_SECS", 8);
    let multiplier_pct = env_u64("PERF_PEAK_MULTIPLIER_PCT", 160);
    let max_error_rate = env_u64("PERF_PEAK_CONTINUE_MAX_ERROR_PCT", 4) as f64 / 100.0;
    let max_p95_ms = env_u64("PERF_PEAK_CONTINUE_MAX_P95_MS", 1200) as f64;
    let max_unhealthy_stages = env_u64("PERF_PEAK_MAX_UNHEALTHY_STAGES", 2);

    let mut stages = Vec::new();
    let mut unhealthy_stages = 0_u64;

    loop {
        let duration = Duration::from_secs(stage_duration_secs);
        let snapshot = run_data_plane_stage(h, id, current as usize, duration).await;
        let healthy = stage_is_healthy(snapshot, max_error_rate, max_p95_ms);
        stages.push(PeakStageReport {
            concurrency: current,
            duration_secs: stage_duration_secs,
            healthy,
            snapshot,
        });

        if healthy {
            unhealthy_stages = 0;
        } else {
            unhealthy_stages = unhealthy_stages.saturating_add(1);
        }

        if current >= max_concurrency || unhealthy_stages >= max_unhealthy_stages {
            break;
        }

        current = next_concurrency(current, max_concurrency, multiplier_pct);
    }

    let max_tested = stages
        .iter()
        .map(|stage| stage.concurrency)
        .max()
        .unwrap_or(0);
    let best = choose_best_peak_stage(&stages);
    (stages, best, max_tested)
}

async fn run_adversarial_stage(
    h: &Harness,
    id: &str,
    forged_concurrency: usize,
    legit_concurrency: usize,
    writer_interval_ms: u64,
    duration: Duration,
) -> AdversarialStageReport {
    let sample_stride = env_u64("PERF_LATENCY_SAMPLE_STRIDE", 16);
    let sample_capacity_hint = env_u64("PERF_LATENCY_SAMPLE_CAPACITY", 200_000) as usize;

    let legit_stats = Arc::new(AtomicStats::new(sample_stride, sample_capacity_hint));
    let forged_total = Arc::new(AtomicU64::new(0));
    let forged_404 = Arc::new(AtomicU64::new(0));
    let forged_transport_error = Arc::new(AtomicU64::new(0));

    let control_write_total = Arc::new(AtomicU64::new(0));
    let control_write_success = Arc::new(AtomicU64::new(0));
    let control_write_error = Arc::new(AtomicU64::new(0));

    let stop = Arc::new(AtomicBool::new(false));
    let start = Instant::now();

    let writer_handle = {
        let h = h.clone();
        let id = id.to_owned();
        let stop = Arc::clone(&stop);
        let total = Arc::clone(&control_write_total);
        let success = Arc::clone(&control_write_success);
        let error = Arc::clone(&control_write_error);
        tokio::spawn(async move {
            let mut version = 0_u64;
            while !stop.load(Ordering::Relaxed) {
                total.fetch_add(1, Ordering::Relaxed);
                let body = format!("payload-version-{version}");
                match h.patch_content(&id, &body).await {
                    Ok(status) if status.is_success() => {
                        success.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(_) | Err(_) => {
                        error.fetch_add(1, Ordering::Relaxed);
                    }
                }
                version = version.wrapping_add(1);
                tokio::time::sleep(Duration::from_millis(writer_interval_ms)).await;
            }
        })
    };

    let mut forged_tasks = Vec::with_capacity(forged_concurrency);
    for worker in 0..forged_concurrency {
        let h = h.clone();
        let stop = Arc::clone(&stop);
        let total = Arc::clone(&forged_total);
        let rejected = Arc::clone(&forged_404);
        let transport = Arc::clone(&forged_transport_error);
        forged_tasks.push(tokio::spawn(async move {
            let mut n = worker as u64;
            while !stop.load(Ordering::Relaxed) {
                let forged = format!("forged-id-{worker}-{n}");
                match h.get_status(&forged).await {
                    Ok(status) => {
                        total.fetch_add(1, Ordering::Relaxed);
                        if status.as_u16() == 404 {
                            rejected.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(_) => {
                        total.fetch_add(1, Ordering::Relaxed);
                        transport.fetch_add(1, Ordering::Relaxed);
                    }
                }
                n = n.wrapping_add(1);
            }
        }));
    }

    let mut legit_tasks = Vec::with_capacity(legit_concurrency);
    for worker in 0..legit_concurrency {
        let h = h.clone();
        let id = id.to_owned();
        let stats = Arc::clone(&legit_stats);
        legit_tasks.push(tokio::spawn(async move {
            let mut n = worker as u64;
            while start.elapsed() < duration {
                let path = if n % 2 == 0 {
                    format!("{id}?port=3000&tenant=prod")
                } else {
                    format!("{id}?port=3000&tenant=prod&noise={n}")
                };
                let t0 = Instant::now();
                match h.get_status(&path).await {
                    Ok(status) => stats.record_status(status, t0.elapsed()).await,
                    Err(_) => stats.record_transport_error(t0.elapsed()).await,
                }
                n = n.wrapping_add(1);
            }
        }));
    }

    for task in legit_tasks {
        task.await.expect("adversarial legit worker panicked");
    }

    stop.store(true, Ordering::Relaxed);

    writer_handle.await.expect("writer task panicked");
    for task in forged_tasks {
        task.await.expect("forged task panicked");
    }

    let legit = legit_stats.snapshot(start.elapsed()).await;
    let forged_total_value = forged_total.load(Ordering::Relaxed);
    let forged_404_value = forged_404.load(Ordering::Relaxed);
    let forged_transport_error_value = forged_transport_error.load(Ordering::Relaxed);
    let forged_rejection_rate = if forged_total_value == 0 {
        0.0
    } else {
        forged_404_value as f64 / forged_total_value as f64
    };

    let control_write_total_value = control_write_total.load(Ordering::Relaxed);
    let control_write_success_value = control_write_success.load(Ordering::Relaxed);
    let control_write_error_value = control_write_error.load(Ordering::Relaxed);
    let control_write_success_rate = if control_write_total_value == 0 {
        0.0
    } else {
        control_write_success_value as f64 / control_write_total_value as f64
    };

    AdversarialStageReport {
        forged_concurrency: forged_concurrency as u64,
        legit_concurrency: legit_concurrency as u64,
        duration_secs: duration.as_secs(),
        healthy: false,
        legit,
        forged_total: forged_total_value,
        forged_404: forged_404_value,
        forged_transport_error: forged_transport_error_value,
        forged_rejection_rate,
        control_write_total: control_write_total_value,
        control_write_success: control_write_success_value,
        control_write_error: control_write_error_value,
        control_write_success_rate,
    }
}

async fn run_adversarial_profile(
    h: &Harness,
    id: &str,
) -> (Vec<AdversarialStageReport>, AdversarialStageReport, u64) {
    let legit_concurrency = env_u64("PERF_ADVERSARIAL_LEGIT_CONCURRENCY", 64) as usize;
    let mut forged_concurrency = env_u64("PERF_ADVERSARIAL_FORGED_INITIAL_CONCURRENCY", 512);
    let max_forged_concurrency = env_u64("PERF_ADVERSARIAL_FORGED_MAX_CONCURRENCY", 32_768);
    let multiplier_pct = env_u64("PERF_ADVERSARIAL_FORGED_MULTIPLIER_PCT", 175);
    let writer_interval_ms = env_u64("PERF_ADVERSARIAL_WRITER_INTERVAL_MS", 120);
    let stage_duration_secs = env_u64("PERF_ADVERSARIAL_STAGE_DURATION_SECS", 8);

    let max_legit_error_rate = env_u64("PERF_ADVERSARIAL_CONTINUE_MAX_ERROR_PCT", 8) as f64 / 100.0;
    let max_legit_p95_ms = env_u64("PERF_ADVERSARIAL_CONTINUE_MAX_P95_MS", 1600) as f64;
    let min_forged_rejection_rate =
        env_u64("PERF_ADVERSARIAL_CONTINUE_MIN_FORGED_REJECTION_PCT", 98) as f64 / 100.0;
    let min_control_write_success_rate = env_u64(
        "PERF_ADVERSARIAL_CONTINUE_MIN_CONTROL_WRITE_SUCCESS_PCT",
        85,
    ) as f64
        / 100.0;
    let max_unhealthy_stages = env_u64("PERF_ADVERSARIAL_MAX_UNHEALTHY_STAGES", 2);

    let mut stages = Vec::new();
    let mut unhealthy_stages = 0_u64;

    loop {
        let duration = Duration::from_secs(stage_duration_secs);
        let mut stage = run_adversarial_stage(
            h,
            id,
            forged_concurrency as usize,
            legit_concurrency,
            writer_interval_ms,
            duration,
        )
        .await;

        stage.healthy = stage.legit.error_rate <= max_legit_error_rate
            && stage.legit.p95_ms <= max_legit_p95_ms
            && stage.forged_rejection_rate >= min_forged_rejection_rate
            && stage.control_write_success_rate >= min_control_write_success_rate;

        if stage.healthy {
            unhealthy_stages = 0;
        } else {
            unhealthy_stages = unhealthy_stages.saturating_add(1);
        }
        stages.push(stage);

        if forged_concurrency >= max_forged_concurrency || unhealthy_stages >= max_unhealthy_stages
        {
            break;
        }

        forged_concurrency =
            next_concurrency(forged_concurrency, max_forged_concurrency, multiplier_pct);
    }

    let max_tested = stages
        .iter()
        .map(|stage| stage.forged_concurrency)
        .max()
        .unwrap_or(0);
    let best = choose_best_adversarial_stage(&stages);
    (stages, best, max_tested)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "heavy performance harness; run in dedicated CI workflow"]
async fn comprehensive_performance_harness() {
    let harness = Harness::start().await;
    let created = harness
        .create(json!({ "content": "Hello {{tenant}} on {{port}}" }))
        .await;
    let id = created["id"].as_str().expect("snippet id").to_owned();

    let (peak_stages, peak_best, peak_max_tested_concurrency) =
        run_peak_profile(&harness, &id).await;
    let (adversarial_stages, adversarial_best, adversarial_max_tested_forged_concurrency) =
        run_adversarial_profile(&harness, &id).await;

    let report = Report {
        peak_stages,
        peak_best,
        peak_max_tested_concurrency,
        adversarial_stages,
        adversarial_best,
        adversarial_max_tested_forged_concurrency,
    };

    let report_json = serde_json::to_string_pretty(&report).expect("serialize report");
    println!("{report_json}");

    let report_path = std::env::var("PERF_RESULTS_PATH")
        .unwrap_or_else(|_| "target/performance-harness-report.json".to_owned());
    if let Some(parent) = std::path::Path::new(&report_path).parent() {
        std::fs::create_dir_all(parent).expect("create report directory");
    }
    std::fs::write(&report_path, report_json).expect("write report file");

    let peak_min_tested_concurrency = env_u64("PERF_ASSERT_PEAK_MIN_TESTED_CONCURRENCY", 2048);
    let peak_min_best_rps = env_u64("PERF_ASSERT_PEAK_MIN_BEST_RPS", 900) as f64;
    let peak_max_best_error_rate = env_u64("PERF_ASSERT_PEAK_MAX_BEST_ERROR_PCT", 8) as f64 / 100.0;

    let adversarial_min_tested_forged_concurrency = env_u64(
        "PERF_ASSERT_ADVERSARIAL_MIN_TESTED_FORGED_CONCURRENCY",
        4096,
    );
    let adversarial_min_legit_rps = env_u64("PERF_ASSERT_ADVERSARIAL_MIN_LEGIT_RPS", 150) as f64;
    let adversarial_max_legit_error_rate =
        env_u64("PERF_ASSERT_ADVERSARIAL_MAX_LEGIT_ERROR_PCT", 15) as f64 / 100.0;
    let min_forged_rejection_rate = env_u64("PERF_ASSERT_FORGED_REJECTION_PCT", 99) as f64 / 100.0;
    let min_control_write_success_rate =
        env_u64("PERF_ASSERT_CONTROL_WRITE_SUCCESS_PCT", 80) as f64 / 100.0;

    assert!(
        report.peak_max_tested_concurrency >= peak_min_tested_concurrency,
        "peak tested concurrency too low: {} < {}",
        report.peak_max_tested_concurrency,
        peak_min_tested_concurrency
    );
    assert!(
        report.peak_best.snapshot.throughput_rps >= peak_min_best_rps,
        "peak best throughput too low: {:.2} rps < {:.2} rps",
        report.peak_best.snapshot.throughput_rps,
        peak_min_best_rps
    );
    assert!(
        report.peak_best.snapshot.error_rate <= peak_max_best_error_rate,
        "peak best error rate too high: {:.4} > {:.4}",
        report.peak_best.snapshot.error_rate,
        peak_max_best_error_rate
    );

    assert!(
        report.adversarial_max_tested_forged_concurrency
            >= adversarial_min_tested_forged_concurrency,
        "adversarial tested forged concurrency too low: {} < {}",
        report.adversarial_max_tested_forged_concurrency,
        adversarial_min_tested_forged_concurrency
    );
    assert!(
        report.adversarial_best.legit.throughput_rps >= adversarial_min_legit_rps,
        "adversarial legit throughput too low: {:.2} rps < {:.2} rps",
        report.adversarial_best.legit.throughput_rps,
        adversarial_min_legit_rps
    );
    assert!(
        report.adversarial_best.legit.error_rate <= adversarial_max_legit_error_rate,
        "adversarial legit error rate too high: {:.4} > {:.4}",
        report.adversarial_best.legit.error_rate,
        adversarial_max_legit_error_rate
    );
    assert!(
        report.adversarial_best.forged_rejection_rate >= min_forged_rejection_rate,
        "forged rejection rate too low: {:.4} < {:.4}",
        report.adversarial_best.forged_rejection_rate,
        min_forged_rejection_rate
    );
    assert!(
        report.adversarial_best.control_write_success_rate >= min_control_write_success_rate,
        "control-plane write success too low: {:.4} < {:.4}",
        report.adversarial_best.control_write_success_rate,
        min_control_write_success_rate
    );
}
