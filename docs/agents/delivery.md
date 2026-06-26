# Data Plane: Delivery, Caching & Rendering

**Read this before working on the Data Plane (`src/delivery/`), the `moka`
cache, or the rendering engine (`src/renderer.rs`).**

The Data Plane is the extreme-throughput delivery half of Serval, bound to
public port **3000**. It serves `GET` requests only, performs template variable
substitution, and is fronted by an in-memory read-through cache. It holds no
telemetry or analytics state — it is intentionally stateless beyond the cache.

## Execution flow (GET only)

1. Parse query-string variables. **Reject requests where `id.len() != 64`.**
2. Look up the `moka` cache for the `(content, content_type, cache_mode)` tuple
   keyed by `id`.
3. On miss, run the index join and store the result in `moka`:
   ```sql
   SELECT c.content, r.content_type, r.cache_mode
   FROM routes r
   INNER JOIN content_blocks c ON c.hash_id = r.target_hash
   WHERE r.id = $1;
   ```
4. Resolve the MIME type from the `*filename` extension via `mime_guess`,
   falling back to the database `content_type`.
5. Render the content with the query variables through `renderer.rs`.
6. Return `200 OK` with a `Cache-Control` header derived from `cache_mode`
   (`0` = mutable/short TTL, `1` = immutable/edge-cached).

## Cache constraints

- Use the **`moka`** crate, asynchronous, bounded to **1,000 entries**
  (~20 MiB). This absorbs read spikes before they reach PostgreSQL.
- The cache is **read-through**; never let it serve stale content after a write.

## Cross-thread invalidation (critical)

A Control Plane `PATCH /api/snippets/{id}` updates the `routes` pointer and MUST
**instantly evict `{id}`** from the Data Plane `moka` cache. Use a cross-thread
message (channel) or the cache's own concurrent eviction API — not a coarse
shared `Mutex` over the whole cache. The very next Data Plane GET for that `id`
must reflect the new content (acceptance criterion #1).

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

- `generate_alias_id() -> String`: 48 CSPRNG bytes → URL-safe Base64, no pad.
- `hash_content(content: &str) -> String`: `SHA3-384(content)` → URL-safe
  Base64, no pad. This single value is both the `content_blocks.hash_id` and the
  `routes.id` of an immutable permalink — so identical text always yields the
  identical permalink URL, regardless of requested extension or MIME type
  (acceptance criterion #3).
