# Data Plane: Delivery, Caching & Rendering

**Read this before working on the Data Plane (`src/delivery/`), the `moka`
cache, or the rendering engine (`src/renderer.rs`).**

The Data Plane is the extreme-throughput delivery half of Serval, bound to
public port **3000**. It serves `GET` requests only, performs template variable
substitution, and is fronted by an in-memory read-through cache. It holds no
telemetry or analytics state — it is intentionally stateless beyond the cache.

## Execution flow (GET only)

1. Parse query-string variables. **Reject requests where `id.len() != 64`.**
2. **Verify the route-id MAC** (`state.signer.verify(id)`). A valid id is
   `prefix || BLAKE3::keyed_hash(key, prefix)[..16]`; a forged or enumerated id
   fails this constant-time check and is rejected with `404` **before any cache
   or database work**. This stateless admission gate is the DoS mitigation — an
   attacker without the deployment secret cannot mint an id that reaches
   PostgreSQL. The rejection is indistinguishable from "not found".
3. Look up the `moka` cache for the `(content, content_type, cache_mode,
   target_hash)` tuple keyed by `id`.
4. On miss, resolve the id in a **single round trip** and store the result in
   `moka`. Both delivery cases are expressed as two primary-key probes under one
   `UNION ALL`, so a live route always wins and the content-addressed path is
   the fallback:
   ```sql
   SELECT c.content, r.content_type, TRUE  AS via_route
   FROM routes r
   JOIN content_blocks c ON c.hash_id = r.target_hash
   WHERE r.id = $1
   UNION ALL
   SELECT c.content, NULL::varchar,     FALSE AS via_route
   FROM content_blocks c
   WHERE c.hash_id = $1;
   ```
   - **Live route** (`via_route = TRUE`) — the id owns a `routes` row; serve its
     current content with the route's `content_type`. It may be repointed by its
     owner, so it is cached as **mutable** (evicted on the owner's next write via
     Control Plane invalidation).
   - **Content-addressed version** (`via_route = FALSE`) — the verified id is
     itself a content hash naming one exact stored block. Serve it directly and
     cache it as **immutable** (it can never change). This is the internal
     "version permalink" path — a deterministic address for a single revision,
     never a separate user-facing snippet kind. A block carries no presentation
     metadata, so the inert default content type is used (a cosmetic filename
     extension can still drive the response MIME).

   The 256-bit id prefix (CSPRNG route id vs. `BLAKE3` content hash) makes the
   two branches collision-free: the query returns **at most one row**, with no
   precedence guard needed. Both branches are unique-index scans on `$1`, so the
   plan is statistics-independent and stable at any data volume.
5. Resolve the MIME type from the `*filename` extension via `mime_guess`,
   falling back to the stored `content_type`.
6. Render the content with the query variables through `renderer.rs`.
7. Return `200 OK` with `ETag` and `Cache-Control` headers derived from **how
   the id resolved**: a live route → `no-cache` (store but revalidate via the
   `ETag`); a content-addressed version → `immutable, max-age=1y`.

## Cache constraints

- Use the **`moka`** crate, asynchronous, byte-budget bounded. The cache
  **never time-evicts** entries — there is no TTL, staleness window, or
  background refresh.
- **Two eviction paths only:**
  1. **Explicit Control Plane invalidation** — `cache.invalidate(id)` on every
     write. This is the sole freshness guarantee (acceptance criterion #1).
  2. **Byte-budget pressure** — moka's LRU weigher evicts the least-recently-used
     entry when total byte weight exceeds `CACHE_BYTE_BUDGET`.
- **Why no TTL is needed.** The Control Plane is the *sole writer* and
  `content_blocks` are immutable, so a cached entry is **provably current**
  between invalidations — there is no second writer and no clock-driven
  divergence to re-discover. A read is a single `moka` `get_or_load`: a hit, or
  one coalesced 1-RTT load on a miss.
- The cache is **read-through**. Correctness depends entirely on Control Plane
  invalidation — never bypass or delay `cache.invalidate(id)` on a write.

## Single-process / cross-thread invalidation (critical)

A Control Plane write — `PATCH /api/snippets/{id}` or
`POST /api/snippets/{id}/restore` — updates the `routes` pointer and MUST
**instantly evict `{id}`** from the Data Plane `moka` cache. Both planes share
**one cache handle in one process** (as `main.rs` wires them), so the eviction
is immediate via the cache's own concurrent API — no channel or coarse `Mutex`.
The very next Data Plane GET for that `id` reflects the new content (acceptance
criterion #1).

Because freshness rests entirely on this in-process invalidation, the two planes
**must not** be split across processes. Doing so would need a notification
channel (e.g. Postgres `LISTEN/NOTIFY`) to carry invalidations between them and
is out of scope. Edge caches handle this via the `no-cache` + `ETag` revalidation
loop, not via the in-process cache.

## Rendering engine (`src/renderer.rs`)

- Compile one global `Regex` for `\{\{([a-zA-Z0-9_]+)\}\}` using
  `std::sync::LazyLock` — compile once, reuse forever.
- Run in strict **O(N)** over the input.
- Replace only keys present in the supplied variables map.
- **Tolerant by design:** leave any unmatched `{{key}}` completely untouched as
  literal text. A request `GET /?port=8080` against a snippet containing
  `{{uuid}}` and `{{port}}` returns the port substituted and the literal
  `{{uuid}}` intact (acceptance criterion #2).

## Cryptography (`src/crypto.rs`)

One hash family — **BLAKE3** — backs both content addressing and the route-id
MAC. Every id is exactly 48 bytes (64 URL-safe Base64 chars, no pad) split into
a **32-byte prefix** and a **16-byte keyed MAC**.

- `IdSigner::new(secret)`: derives the 256-bit MAC key from `ID_SIGNING_SECRET`
  via `blake3::derive_key`. Both planes construct one signer from the same
  secret — the Control Plane mints ids, the Data Plane verifies them.
- `IdSigner::content_id(content) -> String`: `prefix = BLAKE3(content)`, then
  `prefix || MAC(prefix)`. This single value is both the
  `content_blocks.hash_id` and a valid `routes`-shaped id, so identical text
  always yields the identical content address under a fixed secret, regardless
  of requested extension or MIME type. The Data Plane serves it directly as an
  immutable version pointer (acceptance criterion #3).
- `IdSigner::random_id() -> String`: `prefix =` 32 CSPRNG bytes, for a new
  editable snippet route.
- `IdSigner::verify(id) -> bool`: recomputes the MAC over the prefix and
  compares in constant time. The Data Plane calls this before any cache/DB
  lookup; a `false` result is a `404`.

`MAC = BLAKE3::keyed_hash(key, prefix)` truncated to 128 bits. BLAKE3's native
keyed mode is length-extension resistant, so no HMAC wrapper is needed, and
truncation does not weaken the surviving bits — forging a tag still costs
`2^128` work. Rotating `ID_SIGNING_SECRET` (or the `derive_key` context string)
invalidates every existing id deployment-wide.
