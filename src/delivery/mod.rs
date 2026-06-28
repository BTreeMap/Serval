//! The Data Plane: public, extreme-throughput snippet delivery.
//!
//! The hot path is GET-only and deliberately minimal:
//!
//! 1. **Stateless admission gate** — verify the route-id MAC; reject forgeries
//!    before touching the cache or the database.
//! 2. **Immutable shortcut** — if the client already holds the exact
//!    content-addressed version (`If-None-Match: "<id>"`), return `304`
//!    immediately with no cache or database work.
//! 3. **Conditional GET on cache hits** — compute the strong ETag from the
//!    cached `target_hash` and raw query; return `304` if it matches, else
//!    render and serve.  Fresh hits with `If-None-Match` run a cheap step-1
//!    probe (one PK scan on `routes`) so the `304` is never stale.
//! 4. **Opportunistic serve-stale** — stale mutable entries (age >
//!    `refresh_after`) are served immediately while a lock-free single-flight
//!    background task performs a two-step refresh: step-1 checks whether the
//!    hash changed (cheap), step-2 pulls content only if it did. Entries are
//!    **never time-evicted**; the only removal paths are explicit Control Plane
//!    invalidation and byte-budget pressure. When `CACHE_SERVE_STALE=false`
//!    (blocking mode) a stale hit revalidates synchronously before responding.
//! 5. **Miss** — unchanged 1-RTT `fetch_for_delivery` → render → `200`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::cache::{CachedSnippet, DeliveryCache, RefreshGuard};
use crate::db::Repository;
use crate::db::models::{CacheMode, RouteId};
use crate::renderer;
use crate::state::DeliveryState;

/// Build the Data Plane router. Two shapes resolve the same route: a bare id,
/// and an id followed by a cosmetic filename whose extension drives the MIME
/// type. The id itself is never affected by the filename — the content address
/// stays pure.
///
/// Every response carries a hardened, content-agnostic set of security headers.
/// Delivered snippets are untrusted, attacker-influenceable bytes (the query
/// string is reflected into templates), so the public plane refuses to let a
/// browser treat them as an active document: MIME sniffing is disabled, a
/// `default-src 'none'; sandbox` CSP neutralizes any script/embedding, and
/// `no-referrer` keeps the secret-bearing capability URL out of the `Referer`
/// header.
pub fn router(state: DeliveryState) -> Router {
    Router::new()
        .route("/{id}", get(deliver_bare))
        .route("/{id}/{filename}", get(deliver_named))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; sandbox"),
        ))
        .with_state(state)
}

/// Why a delivery load failed. Modeled as an error so the cache never stores a
/// negative result (see [`crate::cache::DeliveryCache::get_or_load`]).
enum LoadError {
    NotFound,
    Database(sqlx::Error),
}

async fn deliver_bare(
    State(state): State<DeliveryState>,
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    deliver(&state, &id, None, query.as_deref(), &headers).await
}

async fn deliver_named(
    State(state): State<DeliveryState>,
    Path((id, filename)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    deliver(&state, &id, Some(&filename), query.as_deref(), &headers).await
}

/// Shared delivery logic for both route shapes.
async fn deliver(
    state: &DeliveryState,
    raw_id: &str,
    filename: Option<&str>,
    query: Option<&str>,
    headers: &HeaderMap,
) -> Response {
    // Reject anything that is not a well-formed 64-char id without touching the
    // database. Indistinguishable from "not found" to avoid id probing.
    let Ok(id) = RouteId::parse(raw_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // Stateless admission gate: a valid id carries a keyed MAC over its prefix.
    // Forged or enumerated ids fail this constant-time check and are rejected
    // here — before any cache or database work — collapsing the DoS
    // amplification vector. Still indistinguishable from "not found".
    if !state.signer.verify(raw_id) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let raw_query = query.unwrap_or("").as_bytes();
    let inm = headers.get(header::IF_NONE_MATCH);
    let ttl_secs = state.refresh_after.as_secs();

    // Immutable shortcut: for content-addressed ids the ETag is just `"<id>"`.
    // If the client already holds that exact validator, return 304 before the
    // cache or database are consulted — the verified id IS the content address,
    // so no further proof is needed.
    let immutable_etag_str = format!("\"{}\"", raw_id);
    if let Some(v) = inm
        && v.to_str()
            .map(|s| s.trim() == immutable_etag_str)
            .unwrap_or(false)
    {
        return build_not_modified(
            str_to_header(&immutable_etag_str),
            cache_control_for(CacheMode::Immutable, ttl_secs, state.serve_stale),
        );
    }

    match state.cache.get_cached(&id).await {
        // ── MISS: unchanged 1-RTT fast path ─────────────────────────────────
        None => {
            let repo = state.repo.clone();
            let load_id = id.clone();
            let loaded = state
                .cache
                .get_or_load(&id, move || async move {
                    match repo.fetch_for_delivery(&load_id).await {
                        Ok(Some(record)) => Ok(CachedSnippet::from(record)),
                        Ok(None) => Err(LoadError::NotFound),
                        Err(e) => Err(LoadError::Database(e)),
                    }
                })
                .await;

            let snippet = match loaded {
                Ok(s) => s,
                Err(e) => return load_error_response(&e),
            };

            let etag = compute_etag(
                raw_id,
                snippet.cache_mode,
                &snippet.target_hash,
                raw_query,
                state,
            );
            if let Some(v) = inm
                && etag_matches(v, &etag)
            {
                return build_not_modified(
                    etag,
                    cache_control_for(snippet.cache_mode, ttl_secs, state.serve_stale),
                );
            }
            build_ok(&snippet, filename, query, raw_query, etag, ttl_secs, state)
        }

        // ── HIT (fresh or stale) ─────────────────────────────────────────────
        Some((snippet, is_stale)) => {
            // For a fresh hit with If-None-Match, run the cheap step-1 probe so
            // the 304 decision is always current. For stale hits (or absent INM)
            // derive the ETag from the cached hash — zero additional DB work.
            let etag = if !is_stale {
                if let Some(v) = inm {
                    // Step-1 probe: one PK scan on `routes`.
                    match state.repo.fetch_target_hash(&id).await {
                        Ok(Some(ref current_hash)) => {
                            let fresh_etag = mutable_etag(current_hash, raw_query, state);
                            if etag_matches(v, &fresh_etag) {
                                return build_not_modified(
                                    fresh_etag,
                                    cache_control_for(
                                        CacheMode::Mutable,
                                        ttl_secs,
                                        state.serve_stale,
                                    ),
                                );
                            }
                            fresh_etag
                        }
                        // Immutable id (no route row) or DB error: fall through
                        // with the cached ETag.
                        Ok(None) | Err(_) => compute_etag(
                            raw_id,
                            snippet.cache_mode,
                            &snippet.target_hash,
                            raw_query,
                            state,
                        ),
                    }
                } else {
                    compute_etag(
                        raw_id,
                        snippet.cache_mode,
                        &snippet.target_hash,
                        raw_query,
                        state,
                    )
                }
            } else {
                compute_etag(
                    raw_id,
                    snippet.cache_mode,
                    &snippet.target_hash,
                    raw_query,
                    state,
                )
            };

            // Stale hit: serve immediately (opportunistic) or revalidate
            // synchronously (blocking), depending on the serve_stale toggle.
            if is_stale {
                if state.serve_stale {
                    // Opportunistic: serve the cached bytes now, fire a
                    // lock-free single-flight refresh in the background. The
                    // RAII guard releases the claim on drop, so the entry can
                    // never be frozen out of future refreshes.
                    if let Some(guard) = RefreshGuard::try_acquire(&snippet) {
                        let repo = state.repo.clone();
                        let cache = state.cache.clone();
                        let bg_id = id.clone();
                        tokio::spawn(background_refresh(repo, cache, bg_id, guard));
                    }
                } else {
                    // Blocking: revalidate inline before responding so the
                    // client always gets a current response.
                    let refreshed = inline_refresh(&state.repo, &state.cache, &id).await;
                    if let Some(fresh) = refreshed {
                        let fresh_etag = compute_etag(
                            raw_id,
                            fresh.cache_mode,
                            &fresh.target_hash,
                            raw_query,
                            state,
                        );
                        if let Some(v) = inm
                            && etag_matches(v, &fresh_etag)
                        {
                            return build_not_modified(
                                fresh_etag,
                                cache_control_for(fresh.cache_mode, ttl_secs, state.serve_stale),
                            );
                        }
                        return build_ok(
                            &fresh, filename, query, raw_query, fresh_etag, ttl_secs, state,
                        );
                    }
                }
            }

            // INM check against the (possibly step-1-refreshed) ETag.
            if let Some(v) = inm
                && etag_matches(v, &etag)
            {
                return build_not_modified(
                    etag,
                    cache_control_for(snippet.cache_mode, ttl_secs, state.serve_stale),
                );
            }

            build_ok(&snippet, filename, query, raw_query, etag, ttl_secs, state)
        }
    }
}

/// Background two-step refresh for a stale cache entry.
///
/// Step-1: cheap hash probe — if the route's `target_hash` is unchanged, just
/// reset the entry's freshness timer (zero data movement). Step-2: only if the
/// hash changed, pull the full content and re-insert. The `RefreshGuard`
/// releases the single-flight claim when this task ends — on success, error, or
/// panic — so the entry can always be re-claimed for a future refresh.
async fn background_refresh(
    repo: Repository,
    cache: DeliveryCache,
    id: RouteId,
    guard: RefreshGuard,
) {
    let entry = guard.entry();
    match repo.fetch_target_hash(&id).await {
        Ok(Some(ref current_hash)) if current_hash.as_str() != entry.target_hash.as_ref() => {
            // Hash changed: step-2 data pull. The replacement entry carries a
            // fresh refresh flag; releasing this (old) entry's claim on drop is
            // harmless because the old entry is no longer reachable.
            match repo.fetch_for_delivery(&id).await {
                Ok(Some(record)) => {
                    cache.insert(&id, record).await;
                }
                Ok(None) => {
                    cache.invalidate(&id).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "background refresh step-2 failed");
                }
            }
        }
        Ok(Some(_)) => {
            // Hash unchanged: reset the freshness timer — zero data movement.
            cache.touch(&id, Arc::clone(entry)).await;
        }
        Ok(None) => {
            // The route no longer exists. Evict the orphaned entry so the next
            // read misses and resolves to a 404 — leaving it cached would serve
            // deleted content indefinitely.
            cache.invalidate(&id).await;
        }
        Err(e) => {
            tracing::warn!(error = %e, "background refresh step-1 failed");
        }
    }
    // `guard` drops here, releasing the single-flight claim.
}

/// Synchronous inline revalidation for the blocking serve-stale=false mode.
///
/// Runs the same two-step probe as the background refresh but on the calling
/// goroutine. Returns the up-to-date snippet (from cache or fresh DB load), or
/// `None` if the route no longer exists.
async fn inline_refresh(
    repo: &Repository,
    cache: &DeliveryCache,
    id: &RouteId,
) -> Option<Arc<CachedSnippet>> {
    match repo.fetch_target_hash(id).await {
        Ok(Some(ref current_hash)) => {
            // Check cached hash to decide if a step-2 pull is needed.
            let cached_hash = cache
                .get_cached(id)
                .await
                .map(|(e, _)| Arc::clone(&e.target_hash));
            if cached_hash.as_deref() != Some(current_hash.as_str()) {
                // Hash changed or not cached: step-2 data pull.
                match repo.fetch_for_delivery(id).await {
                    Ok(Some(record)) => {
                        let fresh = cache.insert(id, record).await;
                        Some(fresh)
                    }
                    Ok(None) => {
                        cache.invalidate(id).await;
                        None
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "inline refresh step-2 failed");
                        // Fall back to stale entry if available.
                        cache.get_cached(id).await.map(|(e, _)| e)
                    }
                }
            } else {
                // Hash unchanged: reset freshness timer, return existing entry.
                if let Some((entry, _)) = cache.get_cached(id).await {
                    cache.touch(id, Arc::clone(&entry)).await;
                    Some(entry)
                } else {
                    None
                }
            }
        }
        Ok(None) => {
            cache.invalidate(id).await;
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "inline refresh step-1 failed");
            cache.get_cached(id).await.map(|(e, _)| e)
        }
    }
}

/// Map a load failure to a response, logging only genuine database errors.
fn load_error_response(err: &Arc<LoadError>) -> Response {
    match err.as_ref() {
        LoadError::NotFound => StatusCode::NOT_FOUND.into_response(),
        LoadError::Database(e) => {
            tracing::error!(error = %e, "data plane delivery query failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Build a `200 OK` response rendering the snippet against the query variables.
fn build_ok(
    snippet: &CachedSnippet,
    filename: Option<&str>,
    query: Option<&str>,
    raw_query: &[u8],
    etag: HeaderValue,
    ttl_secs: u64,
    state: &DeliveryState,
) -> Response {
    let variables = parse_query(query);
    let body = renderer::render(&snippet.content, &variables);
    let content_type = resolve_content_type(filename, &snippet.content_type);
    let cc = cache_control_for(snippet.cache_mode, ttl_secs, state.serve_stale);
    let _ = raw_query; // captured in etag already
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cc),
            (header::ETAG, etag),
        ],
        body,
    )
        .into_response()
}

/// Build a `304 Not Modified` response.
fn build_not_modified(etag: HeaderValue, cache_control: HeaderValue) -> Response {
    let mut resp = StatusCode::NOT_MODIFIED.into_response();
    resp.headers_mut().insert(header::ETAG, etag);
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, cache_control);
    resp
}

/// Compute the strong ETag for a delivery response.
///
/// * **Immutable** ids: `"<id>"` — the id itself is the content address.
/// * **Mutable** routes: `signer.etag(target_hash, raw_query)` — a keyed hash
///   under a key distinct from the route-id MAC, so the serving hash is never
///   derivable from the ETag.
fn compute_etag(
    raw_id: &str,
    mode: CacheMode,
    target_hash: &str,
    raw_query: &[u8],
    state: &DeliveryState,
) -> HeaderValue {
    match mode {
        CacheMode::Immutable => str_to_header(&format!("\"{}\"", raw_id)),
        CacheMode::Mutable => mutable_etag(target_hash, raw_query, state),
    }
}

/// ETag for a mutable route given its `target_hash` and the raw query bytes.
fn mutable_etag(target_hash: &str, raw_query: &[u8], state: &DeliveryState) -> HeaderValue {
    str_to_header(&state.signer.etag(target_hash, raw_query))
}

/// Returns `true` if the client's `If-None-Match` value matches our ETag.
fn etag_matches(inm: &HeaderValue, etag: &HeaderValue) -> bool {
    inm.as_bytes() == etag.as_bytes()
}

/// Parse the query string into template variables. Repeated keys keep the last
/// value; percent-encoding is decoded.
fn parse_query(query: Option<&str>) -> HashMap<String, String> {
    let Some(query) = query else {
        return HashMap::new();
    };
    form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// Resolve the response MIME type: prefer the filename extension, falling back
/// to the stored `content_type`, and finally to inert `text/plain` if the
/// stored value is not a legal header.
///
/// The type is passed through untouched — including `text/html`. Active content
/// is defused by the blanket response headers, not by rewriting the type: every
/// delivery carries `X-Content-Type-Options: nosniff` and a
/// `default-src 'none'; sandbox` CSP, which strips script execution, subresource
/// loads, form submission and same-origin access from any document. A reflected
/// `text/html` snippet is therefore already served inert, so no special case is
/// needed.
fn resolve_content_type(filename: Option<&str>, stored: &str) -> HeaderValue {
    if let Some(name) = filename
        && let Some(guess) = mime_guess::from_path(name).first()
        && let Ok(value) = HeaderValue::from_str(guess.as_ref())
    {
        return value;
    }
    HeaderValue::from_str(stored)
        .unwrap_or_else(|_| HeaderValue::from_static("text/plain; charset=utf-8"))
}

/// Choose a `Cache-Control` policy.
///
/// * **Immutable** ids → `public, max-age=31536000, immutable` (safe to cache
///   forever at the edge; the content never changes).
/// * **Mutable** routes, serve-stale on → `public, max-age=<ttl>,
///   stale-while-revalidate=<ttl>`. The CDN may serve stale for one extra TTL
///   while it revalidates in the background, matching the in-process behaviour.
/// * **Mutable** routes, serve-stale off → `public, max-age=<ttl>,
///   must-revalidate`.
fn cache_control_for(mode: CacheMode, mutable_ttl_secs: u64, serve_stale: bool) -> HeaderValue {
    match mode {
        CacheMode::Immutable => HeaderValue::from_static("public, max-age=31536000, immutable"),
        CacheMode::Mutable => {
            let s = if serve_stale {
                format!(
                    "public, max-age={mutable_ttl_secs}, stale-while-revalidate={mutable_ttl_secs}"
                )
            } else {
                format!("public, max-age={mutable_ttl_secs}, must-revalidate")
            };
            str_to_header(&s)
        }
    }
}

fn str_to_header(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_query_decodes_and_dedupes() {
        let vars = parse_query(Some("port=8080&name=hello%20world&port=9090"));
        assert_eq!(vars.get("name").unwrap(), "hello world");
        assert_eq!(vars.get("port").unwrap(), "9090", "last value wins");
    }

    #[test]
    fn parse_query_handles_none_and_empty() {
        assert!(parse_query(None).is_empty());
        assert!(parse_query(Some("")).is_empty());
    }

    #[test]
    fn content_type_prefers_filename_extension() {
        let ct = resolve_content_type(Some("config.json"), "text/plain; charset=utf-8");
        assert_eq!(ct.to_str().unwrap(), "application/json");
    }

    #[test]
    fn content_type_falls_back_to_stored() {
        let ct = resolve_content_type(None, "text/plain; charset=utf-8");
        assert_eq!(ct.to_str().unwrap(), "text/plain; charset=utf-8");

        // Unknown extension also falls back to the stored type.
        let ct = resolve_content_type(Some("file.unknownext"), "text/markdown");
        assert_eq!(ct.to_str().unwrap(), "text/markdown");
    }

    #[test]
    fn html_is_served_as_is() {
        // The blanket `nosniff` + `default-src 'none'; sandbox` headers render
        // any document inert, so the MIME type is no longer rewritten — HTML
        // passes through whether it comes from the filename or the stored type.
        let ct = resolve_content_type(Some("page.html"), "text/plain; charset=utf-8");
        assert_eq!(ct.to_str().unwrap(), "text/html");

        let ct = resolve_content_type(None, "text/html; charset=utf-8");
        assert_eq!(ct.to_str().unwrap(), "text/html; charset=utf-8");
    }

    #[test]
    fn non_html_types_are_preserved() {
        let ct = resolve_content_type(Some("data.json"), "text/plain");
        assert_eq!(ct.to_str().unwrap(), "application/json");
        let ct = resolve_content_type(None, "image/svg+xml");
        assert_eq!(ct.to_str().unwrap(), "image/svg+xml");
    }

    #[test]
    fn unparseable_stored_type_falls_back_to_text() {
        // A stored value that is not a legal header (e.g. a stray newline)
        // degrades to inert text, not a binary download — this is a text
        // snippet service.
        let ct = resolve_content_type(None, "not\na\nheader");
        assert_eq!(ct.to_str().unwrap(), "text/plain; charset=utf-8");
    }

    #[test]
    fn cache_control_reflects_mutability() {
        assert_eq!(
            cache_control_for(CacheMode::Immutable, 300, false)
                .to_str()
                .unwrap(),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            cache_control_for(CacheMode::Mutable, 300, false)
                .to_str()
                .unwrap(),
            "public, max-age=300, must-revalidate"
        );
        assert_eq!(
            cache_control_for(CacheMode::Mutable, 300, true)
                .to_str()
                .unwrap(),
            "public, max-age=300, stale-while-revalidate=300"
        );
    }

    #[test]
    fn etag_matches_exact_bytes() {
        let a = HeaderValue::from_static("\"abc\"");
        let b = HeaderValue::from_static("\"abc\"");
        let c = HeaderValue::from_static("\"xyz\"");
        assert!(etag_matches(&a, &b));
        assert!(!etag_matches(&a, &c));
    }
}
