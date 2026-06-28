# AGENTS.md

Serval is a high-performance snippet delivery and templating service: a
Rust/Axum backend with a React (Vite + TypeScript) frontend embedded into the
binary via `rust-embed`. It runs a dual-server setup — a Control Plane
(management API + UI) on **8080** and a Data Plane (delivery) on **3000** — over
a pure Content-Addressed Storage (CAS) engine backed by **PostgreSQL only**.

This file is the always-on operating manual for coding agents. It is
intentionally short; load the linked domain docs just-in-time for deeper work.

## Tooling & Commands

- Backend: `cargo` (stable, with `rustfmt` + `clippy`). Frontend: `npm`
  (Node 24). Docker is required for PostgreSQL and the E2E integration suite.
- Build the frontend before the backend — `build.rs` embeds `frontend/dist/`.
- Agent skills live in the `.github/skills` git submodule. Clone with
  `git clone --recurse-submodules`, or run
  `git submodule update --init --recursive` on an existing checkout.

```bash
# Frontend
cd frontend && npm ci && npm run build && npm run lint

# Backend quality gate (matches CI)
cargo fmt --all -- --check
cargo clippy --all-features -- -D warnings -A clippy::too_many_arguments
cargo test
```

Full test / E2E / Dockerized PostgreSQL commands →
[docs/agents/testing.md](docs/agents/testing.md).

## Boundaries & Constraints

- **Never prune `pointer_history`.** It is an infinite, append-only audit
  ledger. Every update appends one row; no pruning, truncation, or retention cap
  is ever applied. → [docs/agents/database.md](docs/agents/database.md).
- **Never break the database to refactor code.** You may rewrite/rename/delete
  Rust interfaces freely, but every schema-affecting change MUST ship a correct,
  idempotent, data-preserving PostgreSQL migration. Data integrity is the one
  hard exception to "ruthless refactoring".
- **Never mutate content blocks.** `content_blocks` is immutable and addressed
  by a signed content id — `Base64URL(BLAKE3(content) || keyed-MAC)`. Insert with
  `ON CONFLICT DO NOTHING`; never update or delete a stored block.
- **Keep content addressing pure.** A content block's id is the signed content
  id: `BLAKE3(content)` as the 32-byte prefix plus a keyed MAC — deterministic
  under the deployment `ID_SIGNING_SECRET` and independent of extension or MIME
  type. This id is an internal, immutable pointer to one exact version (the Data
  Plane serves it directly), **not** a user-facing snippet kind. Every snippet
  is an editable route — do not reintroduce a non-editable "permalink" type or
  derive the content prefix from anything but the content.
- **Never serve an unverified id.** Every Data Plane read MUST pass the keyed-MAC
  check (`IdSigner::verify`) before any cache or database lookup; a failed check
  is a `404`. This stateless gate is the DoS mitigation — do not bypass it or
  move it after the cache. → [docs/agents/delivery.md](docs/agents/delivery.md).
- **Always evict the `moka` cache on writes.** A Control Plane write (`PATCH` or
  restore) MUST invalidate the affected `id` in the Data Plane cache so the next
  GET reflects the change. → [docs/agents/delivery.md](docs/agents/delivery.md).
- **Don't bypass the storage layer.** Persistence goes through the shared pool;
  don't open ad-hoc DB connections inside handlers.
- **Frontend: don't call `fetch`/`axios` directly.** Use the shared API client
  in `frontend/src/`. → [docs/agents/frontend.md](docs/agents/frontend.md).
- **Never edit CI workflows** under `.github/workflows/` unless explicitly
  asked, and never print `DATABASE_URL` or other secrets to logs/CI.

## Definition of Done

Work is complete only when the quality-gate commands above pass, every schema
change is validated against the Dockerized PostgreSQL integration suite, and
tests are added/updated for changed behavior. The four
[acceptance criteria](docs/agents/testing.md#acceptance-criteria) must hold.
The **PR Quality Gate** workflow must be green before review.

## Agent Operating Contract

Every session follows the standards in
[docs/agents/engineering-standards.md](docs/agents/engineering-standards.md).
Key always-on rules:

- **Operate autonomously.** Make the most reasonable assumption on ambiguity,
  document it, and proceed; only pause for destructive/irreversible actions.
  Announce explicit completion — do not stop silently.
- **Make invalid states unrepresentable**; push invariants into the type system.
- **Refactor ruthlessly** (no internal backward-compat duty) and prune dead
  code — except the database, where data integrity is non-negotiable.
- **Keep files ≤ ~500 lines**; split into cohesive submodules as they grow.
- **Handle errors idiomatically** (`Result`/`Option`, `?`, `thiserror`,
  `anyhow`); never swallow them.
- **Avoid reflexive `.clone()`/`Rc`/`Arc`/`Box<dyn _>`** to appease the borrow
  checker — redesign data flow instead. Justified shared ownership (e.g.
  `Arc<Pool>`, the `moka` cache handle) remains fine.

## Agent Skills

Reusable, project-agnostic agent skills are vendored as a **git submodule** at
[.github/skills](.github/skills), tracking
[BTreeMap/SKILLs](https://github.com/BTreeMap/SKILLs). Each skill is a
self-contained `SKILL.md` — load it on demand when its description matches the
task. **Hand-authored commit messages MUST follow the
[git-commits](.github/skills/git-commits/SKILL.md) skill.**

- **Don't edit skills in place.** The submodule is read-only here; propose
  changes upstream in `BTreeMap/SKILLs`, then bump the pointer.
- **Sync skills** by advancing the submodule and committing the new pointer:
  ```bash
  git submodule update --remote .github/skills
  git commit -m "chore(skills): Bump skills submodule"
  ```
- **Fresh checkouts** must init submodules (`git clone --recurse-submodules` or
  `git submodule update --init --recursive`); CI checkouts must pass
  `submodules: recursive`.

## Domain Documentation (load on demand)

| When working on… | Read |
|---|---|
| CAS schema, models, migrations, the history ledger | [docs/agents/database.md](docs/agents/database.md) |
| Data Plane delivery, `moka` cache, rendering | [docs/agents/delivery.md](docs/agents/delivery.md) |
| Tests, CI gate, Dockerized E2E, acceptance criteria | [docs/agents/testing.md](docs/agents/testing.md) |
| React/TypeScript frontend & embedding | [docs/agents/frontend.md](docs/agents/frontend.md) |
| Full engineering standards & output contract | [docs/agents/engineering-standards.md](docs/agents/engineering-standards.md) |
| Reusable agent skills (commit style, authoring) | [.github/skills/README.md](.github/skills/README.md) |
