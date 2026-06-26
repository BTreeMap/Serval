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
| **Data Plane** | `3000` | Public, extreme-throughput delivery. `GET`-only, with template substitution and an in-memory `moka` read-through cache (1,000 entries). |

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

# 2. Build the frontend (build.rs embeds frontend/dist/)
cd frontend && npm ci && npm run build && cd ..

# 3. Run a local PostgreSQL (16+) and point Serval at it
docker compose up -d postgres
export DATABASE_URL=postgres://serval:serval@localhost:5432/serval

# 4. Build and run
cargo run --release
# Control Plane + UI: http://localhost:8080
# Data Plane:         http://localhost:3000
```

## Development

- Quality gate, integration tests, and CI: [docs/agents/testing.md](docs/agents/testing.md)
- Database & migration rules: [docs/agents/database.md](docs/agents/database.md)
- Delivery, caching & rendering internals: [docs/agents/delivery.md](docs/agents/delivery.md)
- Frontend setup: [docs/agents/frontend.md](docs/agents/frontend.md)
- Engineering standards: [docs/agents/engineering-standards.md](docs/agents/engineering-standards.md)

## License

See [LICENSE](LICENSE).
