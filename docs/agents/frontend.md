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
  (create/update/restore); it never talks to the Data Plane. Keep delivery
  stateless.
- **No telemetry/analytics.** The system deliberately omits custom analytics and
  relies on edge network logs. Do not add client-side tracking.
- **Build delivery links via `deliveryUrl(id)`, never by hand.** The Data Plane
  usually lives on a *different domain* than the dashboard, so the base is
  resolved at runtime: the backend advertises `DATA_PLANE_PUBLIC_URL` in the
  `/api/auth-info` bootstrap, which the dashboard records via `setDataPlaneUrl`.
  The helper falls back to the build-time `VITE_DATA_PLANE_URL`, then to a
  `:3000`-on-this-host guess for local dev. Do not reintroduce a hardcoded port
  or origin assumption.
- Let the linter and formatter enforce style — mirror the surrounding code
  rather than hand-tuning formatting.

## Control Plane endpoints the UI uses

- `GET /api/auth-info` — public bootstrap metadata, fetched before sign-in:
  the active auth `mode` and the `data_plane_url` used to build delivery links.
- `POST /api/snippets` — create. Computes `data_hash`, inserts the block
  (`ON CONFLICT DO NOTHING`), generates a CSPRNG `route_id`, and writes version 1
  to `pointer_history`. **Every snippet is editable** — there is no immutable
  snippet kind in the UI.
- `PATCH /api/snippets/{id}` — update. Inserts the new block, repoints the
  route, appends to `pointer_history`, and triggers Data Plane cache eviction
  for `{id}`.
- `GET /api/snippets/{id}` — detail, including the full version `history`.
- `GET /api/snippets/{id}/versions/{hash}` — fetch the content of one past
  version (used to preview a history entry before restoring).
- `POST /api/snippets/{id}/restore` — repoint the snippet to an earlier
  version's `target_hash`; appends a new `pointer_history` row and evicts the
  cache.

The editor is always shown — the detail view lets the user edit the current
content and view or restore any entry in the version history. Internally a
version's `target_hash` is a content address that the Data Plane can serve
directly, but the UI never surfaces it as a separate "permalink" concept; it is
just a pointer to a specific revision in the edit history.
