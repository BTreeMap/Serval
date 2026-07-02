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
- `GET /api/snippets?limit=&cursor=` — paginated listing, newest-changed first.
  Returns `{ snippets, next_cursor, limit }`. `limit` is capped at 50
  (`MAX_PAGE_LIMIT` in `frontend/src/api.ts`); pass the previous page's
  `next_cursor` to fetch the next page. There is no page-number or `OFFSET`
  parameter — the cursor is an opaque, server-signed token, so the UI can only
  move forward one page at a time ("load more"), never jump to an arbitrary page.
- `POST /api/snippets` — create. Computes `data_hash`, inserts the block
  (`ON CONFLICT DO NOTHING`), generates a CSPRNG `route_id`, and writes version 1
  to `pointer_history`. **Every snippet is editable** — there is no immutable
  snippet kind in the UI.
- `PATCH /api/snippets/{id}` — update. Inserts the new block, repoints the
  route, appends to `pointer_history`, and triggers Data Plane cache eviction
  for `{id}`.
- `GET /api/snippets/{id}?limit=` — detail, including only the *newest page* of
  the version ledger. `history_count` is always the exact, unpaginated ledger
  total; `history` holds up to `history_limit` entries; `history_next_cursor` is
  set when older entries remain. Each `HistoryItem` carries a server-computed
  `version_number` and `is_current` — never recompute these from array position
  or length, since only the newest page is ever loaded client-side.
- `GET /api/snippets/{id}/history?limit=&cursor=` — fetch an older page of the
  same route's history, resuming from a `next_cursor` returned by this endpoint
  or by `GET /api/snippets/{id}`. Returns `{ history, next_cursor, limit }`.
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

## Pagination conventions

- Every collection endpoint returns an opaque, signed `next_cursor` string
  (or `null` at the end) instead of an offset or page number. Treat it as a
  black box: store it, echo it back verbatim as `?cursor=`, never parse or
  construct one client-side.
- Cursors are endpoint-specific — a `next_cursor` from `/api/snippets` will be
  rejected with `400` if sent to `/api/snippets/{id}/history`, and vice versa.
  Don't cache or reuse a cursor across a different collection or route id.
  A `400` response should surface as a normal `ApiError`, not be silently retried.
  Restart the affected list from `cursor: undefined` if that ever happens.
- Never request or render more than `MAX_PAGE_LIMIT` (50) rows in one page.
- After a mutation that changes ordering or appends history (create, update,
  restore), refetch from the first page rather than trying to patch an
  in-memory page — `Dashboard.tsx` and `SnippetDetail.tsx` both do this via
  their `refresh()` callbacks.
