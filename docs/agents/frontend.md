# Frontend (React + Vite + TypeScript)

**Read this before working in `frontend/`.** Serval embeds a complete
React/Vite management dashboard directly into the Rust binary. The dashboard is
served by the **Control Plane on port 8080**; the public Data Plane (3000)
serves no UI.

## Build & embedding pipeline

- `build.rs` compiles the Vite/React app in `frontend/` and the binary embeds
  the output (`frontend/dist/`) via a virtual file system (e.g. `rust-embed`).
- **Build the frontend before the backend.** A stale or missing `frontend/dist/`
  embeds outdated assets.

```bash
cd frontend
npm ci          # reproducible install
npm run build   # produces frontend/dist/ for build.rs to embed
npm run lint    # must pass for the quality gate
```

## Conventions

- **Don't call `fetch`/`axios` directly.** Route every Control Plane request
  through the shared API client module under `frontend/src/`, so auth headers,
  base URL, and error handling stay consistent.
- **The dashboard manages snippets only.** It talks to `/api/snippets`
  (create/update); it never talks to the Data Plane. Keep delivery stateless.
- **No telemetry/analytics.** The system deliberately omits custom analytics and
  relies on edge network logs. Do not add client-side tracking.
- Let the linter and formatter enforce style — mirror the surrounding code
  rather than hand-tuning formatting.

## Control Plane endpoints the UI uses

- `POST /api/snippets` — create. Computes `data_hash`, inserts the block
  (`ON CONFLICT DO NOTHING`), generates the `route_id` (CSPRNG alias or hash
  permalink), and writes version 1 to `pointer_history`.
- `PATCH /api/snippets/{id}` — update. Inserts the new block, repoints the
  route, appends to `pointer_history`, and triggers Data Plane cache eviction
  for `{id}`.
