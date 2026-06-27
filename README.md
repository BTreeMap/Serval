# Serval

**Serval is a high-performance snippet delivery and templating engine.** It
stores templated configurations and text snippets once, addresses them by
content hash, and serves them at the edge with on-the-fly variable
substitution over plain HTTP `GET`.

> Building or modifying Serval with an AI coding agent? Start with
> [AGENTS.md](AGENTS.md) and the docs under [docs/agents/](docs/agents/).

## What it does

- **Pure content-addressed storage (CAS).** Raw templates are stored immutably,
  keyed by a signed content id `Base64URL(BLAKE3(content) || keyed-MAC)`. The
  same 20KB config uploaded 1,000 times is stored exactly once — absolute
  byte-level deduplication.
- **Immutable permalinks & mutable aliases.** A permalink's URL is the signed
  content id itself (identical text always yields the identical URL under a
  fixed deployment secret). Aliases are mutable pointers you can repoint over
  time.
- **Forgery-proof ids (DoS mitigation).** Every id carries a keyed MAC over its
  prefix, so the Data Plane rejects forged or enumerated ids with a
  constant-time check before any cache or database lookup. See
  [docs/agents/delivery.md](docs/agents/delivery.md).
- **Tolerant templating.** Snippets use `{{variable}}` placeholders filled from
  standard query parameters (`GET /<id>?port=8080`). Unprovided placeholders are
  left intact as literal text — universally client-compatible, no special
  request format required.
- **Versioned history.** Every alias update is recorded in an infinite,
  append-only ledger so prior content states stay auditable.
- **Embedded dashboard.** A React/Vite management UI is bundled into the binary;
  no separate frontend deployment needed.

## Architecture

Serval runs two HTTP servers over a single PostgreSQL database:

| Plane | Port | Role |
|---|---|---|
| **Control Plane** | `8080` | Management API (`/api/snippets`) + embedded React dashboard. Handles version-controlled writes. |
| **Data Plane** | `3000` | Public, extreme-throughput delivery. `GET`-only, with template substitution and a byte-bounded in-memory `moka` read-through cache. |

A `PATCH` on the Control Plane instantly evicts the affected entry from the Data
Plane cache, so updates are reflected on the very next read.

### Storage layout

- `content_blocks` — deduplicated, immutable raw payloads, addressed by hash.
- `routes` — active aliases/permalinks pointing at a content hash, with MIME
  type and cache mode.
- `pointer_history` — append-only audit ledger of every pointer change.

See [docs/agents/database.md](docs/agents/database.md) for the full schema.

## Tech stack

- **Backend:** Rust (Axum), PostgreSQL 16+, `moka` cache, BLAKE3 hashing &
  keyed-MAC route ids.
- **Frontend:** React + Vite + TypeScript, embedded via `rust-embed` at build
  time.
- **Delivery:** stateless by design — no custom telemetry; rely on edge logs.

## Getting started

```bash
# 1. Init submodules (agent skills live in .github/skills)
git submodule update --init --recursive

# 2. Run a local PostgreSQL (16+) and point Serval at it
docker compose up -d postgres
export DATABASE_URL=postgres://serval:serval@localhost:5432/serval

# Set the route-id signing secret (required; >= 32 chars, keep it stable)
export ID_SIGNING_SECRET="$(openssl rand -base64 48)"

# 3. Build and run (build.rs builds and embeds frontend/dist/ automatically)
cargo run --release
# Control Plane + UI: http://localhost:8080
# Data Plane:         http://localhost:3000
```

The database schema is applied idempotently on startup, so the first run needs
no separate migration step. To run the whole stack — app included — in
containers instead:

```bash
# Pull a published image and run the full stack (uses :stable by default)
docker compose --profile app up

# Or build the app from your local checkout instead of pulling
docker compose --profile build up --build
```

## Container images

Multi-arch images (`linux/amd64` + native `linux/arm64`) are published to the
GitHub Container Registry at `ghcr.io/btreemap/serval`. Two rolling channels
make the stability contract explicit:

| Tag | Channel | When it moves |
|---|---|---|
| `:stable` | Vetted releases | Published from each `v*` version tag. Recommended for real deployments. |
| `:latest` | Cutting edge | Republished on every `main` build **after the PR Quality Gate passes**, so community testers can exercise unreleased changes without running un-vetted code. |
| `:vX.Y.Z`, `:vX.Y` | Pinned release | Immutable semantic-version tags for reproducible deploys. |

```bash
docker pull ghcr.io/btreemap/serval:stable   # production
docker pull ghcr.io/btreemap/serval:latest   # help us test main
```

Point `docker compose` at a channel with `SERVAL_IMAGE_TAG` (defaults to
`stable`):

```bash
SERVAL_IMAGE_TAG=latest docker compose --profile app up
```

## Using the API

The Control Plane speaks JSON under `/api`. With the default `AUTH_MODE=none`
every request is the local superuser, so no token is needed for local use.

```bash
# Create a mutable alias (random id) holding a template
curl -s -X POST http://localhost:8080/api/snippets \
  -H 'content-type: application/json' \
  -d '{"content":"Hello {{name}} on {{port}}","immutable":false}'
# => {"id":"<alias>","immutable":false, ...}

# Fetch it from the Data Plane, substituting variables from the query string
curl "http://localhost:3000/<alias>?name=world&port=8080"
# => Hello world on 8080   ({{name}}/{{port}} filled; unknown vars stay literal)

# Publish a new version (mutable aliases only); the next GET reflects it
curl -s -X PATCH http://localhost:8080/api/snippets/<alias> \
  -H 'content-type: application/json' -d '{"content":"Goodbye"}'

# Create an immutable permalink: the id *is* the content hash
curl -s -X POST http://localhost:8080/api/snippets \
  -H 'content-type: application/json' \
  -d '{"content":"pinned forever","immutable":true}'
```

When `AUTH_MODE=oauth`, send `Authorization: Bearer <jwt>`; identity comes from
the token's `sub`, while the **admin** role is managed locally (see below).

## Configuration

Serval is configured entirely through the environment (a local `.env` is loaded
if present). See [.env.example](.env.example) for the full list.

| Variable | Default | Purpose |
|---|---|---|
| `DATABASE_URL` | _required_ | PostgreSQL connection string |
| `ID_SIGNING_SECRET` | _required_ | Secret salt (>= 32 chars) keying the route-id MAC; keep stable per deployment |
| `DATABASE_MAX_CONNECTIONS` | `16` | Pool size |
| `CONTROL_PLANE_ADDR` | `0.0.0.0:8080` | Management API + UI bind address |
| `DATA_PLANE_ADDR` | `0.0.0.0:3000` | Delivery bind address |
| `CACHE_BYTE_BUDGET` | `33554432` | Delivery cache size cap, in bytes |
| `CACHE_MUTABLE_TTL_SECS` | `300` | TTL for mutable aliases in the cache |
| `AUTH_MODE` | `none` | `none` (local superuser) or `oauth` |
| `OAUTH_ISSUER` / `OAUTH_AUDIENCE` / `OAUTH_JWKS_URL` | — | Required when `AUTH_MODE=oauth` |

## Admin roles (CLI)

Authorization for writes is owner-or-admin, and the admin role lives in
Serval's own `users` table rather than in any token claim. Manage it out of band
with the CLI:

```bash
serval admin promote <user-id>   # grant the admin role
serval admin demote  <user-id>   # revoke it
serval admin list                # list current admins
```

## Development

- Quality gate, integration tests, and CI: [docs/agents/testing.md](docs/agents/testing.md)
- Database & migration rules: [docs/agents/database.md](docs/agents/database.md)
- Delivery, caching & rendering internals: [docs/agents/delivery.md](docs/agents/delivery.md)
- Frontend setup: [docs/agents/frontend.md](docs/agents/frontend.md)
- Engineering standards: [docs/agents/engineering-standards.md](docs/agents/engineering-standards.md)

### CI/CD security & caching

The GitHub Actions pipeline is built for least privilege and supply-chain
containment:

- **Deny-all token default.** Every workflow sets `permissions: {}` at the top
  and grants each job only the narrowest scope it needs.
- **Build code never holds a write token.** Jobs that execute third-party code
  (`npm install`, `cargo build` scripts/proc-macros) run read-only. Publishing
  is split into separate jobs — releases are attached and images are pushed by
  steps that run **no** build code — so a poisoned dependency has no
  release/registry token to exfiltrate.
- **`:latest` is gated.** Images are pushed to `:latest` only after the PR
  Quality Gate passes on `main`; the publish workflow never runs on pull
  requests, so untrusted PR code never sees a registry-write token.
- **Poison-proof compiler cache.** Rust caching is shared for speed, but only
  builds on `main` may *write* the cache while pull requests restore it
  read-only — a PR cannot persist a poisoned artifact for a later trusted build.

## License

See [LICENSE](LICENSE).
