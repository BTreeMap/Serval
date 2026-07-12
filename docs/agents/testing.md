# Testing, CI & Acceptance Criteria

**Read this before changing behavior, schema, or CI.** Serval has no SQLite
path: all integration tests run against a real, ephemeral PostgreSQL 16+
instance booted via Docker Compose or `testcontainers-rs`.

## Local quality gate

```bash
# Backend (matches the PR Quality Gate workflow)
cargo fmt --all -- --check
cargo clippy --all-features -- -D warnings -A clippy::too_many_arguments
cargo test

# Frontend
cd frontend && npm ci && npm run build && npm run lint
```

## Integration / E2E suite

The integration tests compile the Serval binary and run E2E checks against a
live database to verify routing, cross-thread cache invalidation, and header
correctness. Bring up an ephemeral PostgreSQL before running them:

```bash
# Option A: testcontainers-rs manages the container in-process
cargo test --test '*'

# Option B: explicit Docker Compose Postgres, then point Serval at it
docker compose up -d postgres
export DATABASE_URL=postgres://serval:serval@localhost:5432/serval
cargo test --test '*'
```

> Per environment policy, do not run Playwright/Chromium browser E2E locally;
> rely on CI for those. The Serval E2E suite is Bash + HTTP against the two
> servers and is safe to run locally.

## CI workflows (`.github/workflows/`)

| Workflow | Responsibility |
|---|---|
| `pr-quality-gate.yml` | `cargo fmt`, `clippy`, and UI linting |
| `integration-tests.yml` | Boots ephemeral PostgreSQL 16+, builds the binary, runs E2E Bash tests for routing, invalidation, and headers |
| `performance-harness.yml` | Runs an extreme-load release harness that enforces latency/throughput/error-rate SLOs, plus an independent symbolized profiling job that uploads Peak and Adversarial flamegraphs |
| `build-binaries.yml` | Cross-compiles for Linux/macOS/Windows |
| `docker-publish.yml` | Builds minimal `distroless`/`scratch` OCI images |

Do not edit these workflows unless explicitly asked. CI checkouts must pass
`submodules: recursive` so the `.github/skills` submodule is present.

## Performance harness

The repository ships a dedicated heavy-load harness at
`tests/performance_harness.rs` (feature-gated with `integration` and ignored by
default so it does not impact standard local `cargo test` runs).

It validates two traffic classes:

1. **Peak traffic profile.** Sustained, high-concurrency legitimate GETs to
   the Data Plane with realistic query substitutions.
2. **Misaligned actor profile.** Concurrent legitimate reads while forged-id
   floods and rapid Control Plane writes run in parallel.

Run locally with the same command CI uses:

```bash
cargo test --release --features integration --test performance_harness -- --ignored --nocapture
```

Tune load and assertion thresholds via `PERF_*` environment variables (see the
workflow for the canonical set). The harness writes
`target/performance-harness-report.json` by default.

The workflow's separate profiling job uses fixed representative workloads to
produce interactive CPU flamegraphs without changing the stripped release
profile or its SLO measurements. See [profiling.md](profiling.md) for the
architecture, configuration, local command, and interpretation guidance.

## Acceptance criteria

Every change must keep all four of these true, verified via the Dockerized
PostgreSQL integration tests:

1. **Cross-thread invalidation.** Updating a snippet via the Control Plane
   (`PATCH` or restore) is reflected on the very next Data Plane GET — proving
   the `moka` cache was evicted.
2. **Tolerant rendering.** `GET /?port=8080` for a snippet containing `{{uuid}}`
   and `{{port}}` returns the port substituted and the literal `{{uuid}}`
   intact.
3. **Content-addressed delivery.** A version's content hash is the signed
   content id `Base64URL(BLAKE3(content) || keyed-MAC)` — identical text always
   yields the identical hash under a fixed deployment secret, regardless of
   extension or MIME type. The Data Plane serves that hash directly as an
   immutable version (long-lived `Cache-Control: immutable`), while it rejects
   any id whose MAC fails verification with a `404`, before any cache/DB work.
4. **Infinite ledger.** Modifying a snippet 100 times yields exactly 101
   `pointer_history` rows, with no pruning.
