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
| `build-binaries.yml` | Cross-compiles for Linux/macOS/Windows |
| `docker-publish.yml` | Builds minimal `distroless`/`scratch` OCI images |

Do not edit these workflows unless explicitly asked. CI checkouts must pass
`submodules: recursive` so the `.github/skills` submodule is present.

## Acceptance criteria

Every change must keep all four of these true, verified via the Dockerized
PostgreSQL integration tests:

1. **Cross-thread invalidation.** Updating a mutable alias via the Control Plane
   is reflected on the very next Data Plane GET — proving the `moka` cache was
   evicted.
2. **Tolerant rendering.** `GET /?port=8080` for a snippet containing `{{uuid}}`
   and `{{port}}` returns the port substituted and the literal `{{uuid}}`
   intact.
3. **Untyped permalink purity.** Immutable permalink creation yields a 64-char
   id derived only from `Base64URL(SHA3-384(content))` — identical text always
   gives the identical URL, regardless of extension or MIME type.
4. **Infinite ledger.** Modifying an alias 100 times yields exactly 101
   `pointer_history` rows, with no pruning.
