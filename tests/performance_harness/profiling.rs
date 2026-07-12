use super::*;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProfileScenario {
    Peak,
    Adversarial,
}

impl ProfileScenario {
    const fn file_name(self) -> &'static str {
        match self {
            Self::Peak => "peak.svg",
            Self::Adversarial => "adversarial.svg",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Peak => "peak",
            Self::Adversarial => "adversarial",
        }
    }
}

#[derive(Debug)]
struct FlamegraphConfig {
    output_dir: PathBuf,
    frequency_hz: i32,
    duration: Duration,
    peak_concurrency: NonZeroUsize,
    adversarial_forged_concurrency: NonZeroUsize,
    adversarial_legit_concurrency: NonZeroUsize,
    writer_interval_ms: NonZeroU64,
}

impl FlamegraphConfig {
    fn from_env() -> Self {
        let frequency = env_nonzero_u64("PERF_FLAMEGRAPH_FREQUENCY_HZ", 99);
        let frequency_hz = i32::try_from(frequency.get())
            .ok()
            .filter(|frequency| *frequency > 0)
            .expect("PERF_FLAMEGRAPH_FREQUENCY_HZ must fit in a positive i32");

        Self {
            output_dir: std::env::var_os("PERF_FLAMEGRAPH_OUTPUT_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("target/flamegraphs")),
            frequency_hz,
            duration: Duration::from_secs(
                env_nonzero_u64("PERF_FLAMEGRAPH_DURATION_SECS", 15).get(),
            ),
            peak_concurrency: env_nonzero_usize("PERF_FLAMEGRAPH_PEAK_CONCURRENCY", 512),
            adversarial_forged_concurrency: env_nonzero_usize(
                "PERF_FLAMEGRAPH_ADVERSARIAL_FORGED_CONCURRENCY",
                2_048,
            ),
            adversarial_legit_concurrency: env_nonzero_usize(
                "PERF_FLAMEGRAPH_ADVERSARIAL_LEGIT_CONCURRENCY",
                64,
            ),
            writer_interval_ms: env_nonzero_u64("PERF_FLAMEGRAPH_WRITER_INTERVAL_MS", 120),
        }
    }

    fn output_path(&self, scenario: ProfileScenario) -> PathBuf {
        self.output_dir.join(scenario.file_name())
    }
}

fn env_nonzero_u64(key: &str, default: u64) -> NonZeroU64 {
    let value = match std::env::var(key) {
        Ok(raw) => raw
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("{key} must be a positive integer, got {raw:?}")),
        Err(std::env::VarError::NotPresent) => default,
        Err(std::env::VarError::NotUnicode(_)) => panic!("{key} must contain valid UTF-8"),
    };
    NonZeroU64::new(value).unwrap_or_else(|| panic!("{key} must be greater than zero"))
}

fn env_nonzero_usize(key: &str, default: usize) -> NonZeroUsize {
    let value = env_nonzero_u64(key, default as u64).get();
    NonZeroUsize::new(
        usize::try_from(value).unwrap_or_else(|_| panic!("{key} does not fit in usize")),
    )
    .expect("value was already checked as non-zero")
}

struct ProfileSession {
    scenario: ProfileScenario,
    output_path: PathBuf,
    guard: pprof::ProfilerGuard<'static>,
}

impl ProfileSession {
    fn start(config: &FlamegraphConfig, scenario: ProfileScenario) -> Self {
        let output_path = config.output_path(scenario);
        let guard = pprof::ProfilerGuard::new(config.frequency_hz)
            .unwrap_or_else(|error| panic!("start {} profiler: {error}", scenario.label()));
        Self {
            scenario,
            output_path,
            guard,
        }
    }

    fn finish(self) {
        let report = self
            .guard
            .report()
            .build()
            .unwrap_or_else(|error| panic!("build {} profile: {error}", self.scenario.label()));
        drop(self.guard);

        let parent = self.output_path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("create flamegraph directory {}: {error}", parent.display())
        });
        let file = std::fs::File::create(&self.output_path).unwrap_or_else(|error| {
            panic!(
                "create {} flamegraph {}: {error}",
                self.scenario.label(),
                self.output_path.display()
            )
        });
        report.flamegraph(file).unwrap_or_else(|error| {
            panic!(
                "write {} flamegraph {}: {error}",
                self.scenario.label(),
                self.output_path.display()
            )
        });
        eprintln!(
            "[profile] wrote {} flamegraph to {}",
            self.scenario.label(),
            self.output_path.display()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "CPU profiling harness; run in dedicated CI workflow"]
async fn representative_hot_path_flamegraphs() {
    let config = FlamegraphConfig::from_env();
    let harness = Harness::start().await;
    let created = harness
        .create(json!({ "content": "Hello {{tenant}} on {{port}} in {{region}}" }))
        .await;
    let id = created["id"].as_str().expect("snippet id").to_owned();

    let warm_path = format!("{id}?tenant=prod&port=8080&region=us-east");
    let warm_status = harness
        .get_status(&warm_path)
        .await
        .expect("warm-up request");
    assert!(warm_status.is_success(), "warm-up failed: {warm_status}");

    eprintln!(
        "[profile] peak: {}s at c{} and {} Hz",
        config.duration.as_secs(),
        config.peak_concurrency,
        config.frequency_hz
    );
    let peak = ProfileSession::start(&config, ProfileScenario::Peak);
    let peak_snapshot = run_data_plane_stage(
        &harness,
        &id,
        config.peak_concurrency.get(),
        config.duration,
    )
    .await;
    peak.finish();
    assert!(
        peak_snapshot.total > 0,
        "peak profile completed no requests"
    );

    eprintln!(
        "[profile] adversarial: {}s at forged c{}, legit c{}, writer interval {}ms and {} Hz",
        config.duration.as_secs(),
        config.adversarial_forged_concurrency,
        config.adversarial_legit_concurrency,
        config.writer_interval_ms,
        config.frequency_hz
    );
    let adversarial = ProfileSession::start(&config, ProfileScenario::Adversarial);
    let adversarial_report = run_adversarial_stage(
        &harness,
        &id,
        config.adversarial_forged_concurrency.get(),
        config.adversarial_legit_concurrency.get(),
        config.writer_interval_ms.get(),
        config.duration,
    )
    .await;
    adversarial.finish();
    assert!(
        adversarial_report.forged_total > 0 && adversarial_report.legit.total > 0,
        "adversarial profile did not exercise both traffic classes"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenarios_have_stable_distinct_output_names() {
        assert_eq!(ProfileScenario::Peak.file_name(), "peak.svg");
        assert_eq!(ProfileScenario::Adversarial.file_name(), "adversarial.svg");
        assert_ne!(
            ProfileScenario::Peak.file_name(),
            ProfileScenario::Adversarial.file_name()
        );
    }
}
