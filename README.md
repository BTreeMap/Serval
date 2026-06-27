# Serval

**Serval is a high-performance snippet delivery and templating engine.** It
stores templated configurations and text snippets once, addresses them by
content hash, and serves them at the edge with on-the-fly variable
substitution over plain HTTP `GET`.

> Building or modifying Serval with an AI coding agent? Start with
> [AGENTS.md](AGENTS.md) and the docs under [docs/agents/](docs/agents/).

## What it does

- **Pure content-addressed storage (CAS).** Raw templates are stored immutably,
  keyed by `Base64URL(SHA3-384(content))`. The same 20KB config uploaded 1,000
  times is stored exactly once — absolute byte-level deduplication.
- **Immutable permalinks & mutable aliases.** A permalink's URL is the content
  hash itself (identical text always yields the identical URL). Aliases are
  mutable pointers you can repoint over time.
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

- **Backend:** Rust (Axum), PostgreSQL 16+, `moka` cache, SHA3-384 hashing.
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

# 3. Build and run (build.rs builds and embeds frontend/dist/ automatically)
cargo run --release
# Control Plane + UI: http://localhost:8080
# Data Plane:         http://localhost:3000
```

The database schema is applied idempotently on startup, so the first run needs
no separate migration step. To run the whole stack — app included — in
containers instead:

```bash
docker compose --profile app up --build
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

## License

See [LICENSE](LICENSE).
