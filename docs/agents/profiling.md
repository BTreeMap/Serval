# CPU Profiling & Flamegraphs

Serval collects CPU flamegraphs for representative Data Plane workloads in the
**Performance Harness** workflow. Profiling is diagnostic: the existing
stripped-release performance harness remains the source of truth for
throughput, latency, error-rate, and rejection SLOs.

## Architecture

The profiling test reuses the PostgreSQL-backed harness and its real Axum
Control Plane, Data Plane, `moka` cache, renderer, signer, and HTTP clients. It
captures two independent workloads:

1. **Peak:** sustained mutable-route GETs against a warm cache, including query
   substitution and varying query strings.
2. **Adversarial:** legitimate rendered GETs alongside forged-id rejection and
   periodic Control Plane writes/cache invalidations.

Each workload owns one `pprof` sampling guard and produces a separate
interactive SVG. Because the servers and load generators run in one process,
the graphs include both sides of the loopback request. Follow call trees rooted
in `serval::delivery`, `serval::crypto`, `serval::cache`, or
`serval::renderer` to focus on server cost.

`pprof` was selected instead of `perf`/`cargo-flamegraph` because it captures
exact workload boundaries in process and does not require root, `sudo`, or a
runner-specific `perf_event_paranoid` change. CI forces frame pointers for the
profiling job to improve stack recovery.

## Production isolation

Profiling is absent unless the `profiling` Cargo feature is explicitly enabled.
The dedicated `profiling` Cargo profile inherits release optimization but keeps
line tables and symbols. The normal `release` profile remains stripped, and
production binaries neither compile nor link `pprof`.

## Running locally

A Docker daemon is required because the harness starts PostgreSQL through
`testcontainers`. On a Linux host with Docker available, run:

```bash
SERVAL_SKIP_FRONTEND_BUILD=1 \
RUSTFLAGS="-C force-frame-pointers=yes" \
cargo test --profile profiling --locked \
  --features integration,profiling \
  --test performance_harness \
  representative_hot_path_flamegraphs \
  -- --ignored --nocapture
```

The command writes:

- `target/flamegraphs/peak.svg`
- `target/flamegraphs/adversarial.svg`

Open either SVG in a browser. Hover over frames to inspect sample share, click
to zoom, and use the graph search control to find symbols such as
`IdSigner::verify`, delivery cache operations, query rendering, and SQLx.

## Configuration

All workload settings must be positive integers. Invalid values fail before the
profiler or load workers start.

| Variable | Default | Meaning |
|---|---:|---|
| `PERF_FLAMEGRAPH_OUTPUT_DIR` | `target/flamegraphs` | SVG output directory |
| `PERF_FLAMEGRAPH_FREQUENCY_HZ` | `99` | Samples per second |
| `PERF_FLAMEGRAPH_DURATION_SECS` | `15` | Duration of each scenario |
| `PERF_FLAMEGRAPH_PEAK_CONCURRENCY` | `512` | Peak GET workers |
| `PERF_FLAMEGRAPH_ADVERSARIAL_FORGED_CONCURRENCY` | `2048` | Forged-id workers |
| `PERF_FLAMEGRAPH_ADVERSARIAL_LEGIT_CONCURRENCY` | `64` | Legitimate GET workers during adversarial traffic |
| `PERF_FLAMEGRAPH_WRITER_INTERVAL_MS` | `120` | Delay between Control Plane writes |

The common performance harness variables `PERF_REQUEST_TIMEOUT_SECS`,
`PERF_LATENCY_SAMPLE_STRIDE`, and `PERF_LATENCY_SAMPLE_CAPACITY` also apply.

## CI results

The `Representative hot-path flamegraphs` job runs independently from the
release SLO job on manual dispatch, the nightly schedule, and relevant pushes
to `main`. Its GitHub job summary links a 14-day `hot-path-flamegraphs` artifact
containing both SVGs and records the exact CI workload settings.

Flamegraphs identify where sampled CPU time is spent; they are not stable
performance thresholds. Confirm an optimization with the stripped-release
performance harness and its JSON report rather than comparing SVG widths
across noisy shared runners.
