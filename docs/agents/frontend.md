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
npm ci           # reproducible install
npm run verify   # lint + test + build, in that order — the only entry point
```

`verify` is `lint && test && build` folded into one script, and **producing
`frontend/dist/` without checking it must stay impossible.** Every CI path goes
through one of exactly two front doors, both of which run this fold:

| Path | Front door |
|---|---|
| `pr-quality-gate.yml`, `build-binaries.yml` | [.github/actions/frontend](../../.github/actions/frontend/action.yml) |
| `Dockerfile` stage 1 (used by docker-publish, integration tests) | `npm run verify` |

A Dockerfile cannot call a composite action, which is why the fold also lives in
`package.json` — keep the two in step. Do not add a bare `npm run build` step to
a workflow: adding the missing checks at each call site is what lets them drift
apart again. `build.rs` still runs a plain `npm run build`, because its job is to
produce the bundle for embedding during local development; every CI path that
*ships* a bundle verifies it first.

Individual scripts (`npm run lint`, `npm test`, `npm run build`) remain for
tight local loops.

## Tests

`npm test` runs [scripts/run-suite.mjs](../../frontend/scripts/run-suite.mjs),
a thin wrapper over Node 24's built-in runner. It exists for one reason:
**`node --test` exits 0 when it discovers no test files.** Since `verify` chains
`lint && test && build`, a suite that silently stopped being found — a rename, a
moved directory, a change in Node's discovery patterns — would let every workflow
and the Docker image report a green frontend while checking none of it. The
wrapper asserts the suite is non-empty first, then invokes `node --test` with no
arguments so there is no shell glob for a Windows or macOS runner to expand.

Do not name a helper in a way that matches Node's own discovery patterns
(`test.mjs`, `*.test.mjs`, `*-test.mjs`, `test-*.mjs`, or anything under a
`test/` directory) — the runner will execute it as a test case.

The suite covers two kinds of file:

- `src/lib/*.test.ts` — the pure core. Framework-free, so it needs no DOM, no
  renderer and no test framework beyond the standard library.
- `src/public-assets.test.ts` — anything under `public/`, which is copied
  byte-for-byte into `dist/`. Those files are **unverified by definition**: no
  build step parses them, so typecheck, lint and build can all be green while
  the shipped asset renders a browser parser-error page. A `--` inside an XML
  comment in `favicon.svg` is the concrete way this happens.

When adding an assertion, reintroduce the defect it describes and confirm the
test goes red. A test never seen fail is a hypothesis, not a guard — and assert
that any glob or file list is non-empty, or every case below it passes
vacuously.

## The pure core (`src/lib/`)

Framework-free, directly testable modules that carry the app's domain vocabulary.
Components are the presentation of these; the rules live here, once.

- `remote-data.ts` — `RemoteData<T>`: the `loading | success | failure` union that
  replaced every `data` + `loading` + `error` cell trio. `failure` carries the
  last good value (`stale`), which is what lets a list stay on screen under an
  error banner after a failed revalidation. Eliminate it with `foldRemote`.
- `pagination.ts` — `Page<T>` plus the pure cursor-pagination fold
  (`concatPages`, `cursorAfter`, `loadedCount`). Ordered, non-commutative.
- `useRemoteQuery.ts` — the effect interpreter for a keyed read. `loading` is
  **derived** from whether the newest request has settled, never stored, and the
  effect aborts on cleanup so a stale response cannot land. Its `refresh()`
  revalidates without clearing the screen and resolves once the new value is
  committed, so a mutation can await a consistent view.
- `useCursorPager.ts` — appends further pages to a seed the query owns. Pages are
  tagged with the seed they extend, so an append that resolves after a refresh
  cannot land on the new collection.
- `useInlineEdit.ts` — the `viewing | editing | saving | failed` state machine
  shared by every inline editor.
- `errors.ts` — `messageOf`: only an `ApiError` message reaches the user.
- `format.ts` — one hoisted `Intl.DateTimeFormat`, not one per row per render.

Rules for this directory:

- **Nothing in `src/lib/` may import from outside `src/lib/`,** and imports
  within it carry the `.ts` extension, so the core runs unbundled under
  `node --test`. That is the whole reason `ApiError` lives in `lib/api-error.ts`
  and is re-exported from `api.ts`.
- **Derive, don't store.** If a value is a function of other state (`loading`,
  a filtered list, an error parsed from the URL), compute it during render.
  Two cells that must agree are two cells that can disagree.
- **A `react-hooks` complaint is design feedback, not noise.** All three former
  `eslint-disable react-hooks/set-state-in-effect` comments were removed by
  changing the state shape, not by suppressing the rule. Do not reintroduce one.

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
  the `refresh()` their `useRemoteQuery` returns. That refetch replaces the
  seed page, which is also what discards any previously appended pages; there
  is no separate reset to keep in sync.
- **A revalidation must not be answered from the prefetch cache.** A refetch
  after a write exists to observe that write, and a link warmed seconds earlier
  predates it. `useRemoteQuery` passes `isRevalidation` to its loader for
  exactly this; consult it rather than calling `loadPrefetched` unconditionally.
