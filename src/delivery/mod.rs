//! The Data Plane: public, extreme-throughput snippet delivery.
//!
//! The hot path is GET-only and deliberately minimal:
//!
//! 1. **Stateless admission gate** — verify the route-id MAC; reject forgeries
//!    before touching the cache or the database.
//! 2. **Immutable shortcut** — if the client already holds the exact
//!    content-addressed version (`If-None-Match: "<id>"`), return `304`
//!    immediately with no cache or database work.
//! 3. **Read-through cache** — a single [`DeliveryCache::get_or_load`] resolves
//!    a hit or coalesces concurrent misses into one 1-RTT `fetch_for_delivery`.
//!    A present entry is always current: the Control Plane invalidates it on
//!    every write, so there is no TTL, staleness window, or background refresh.
//! 4. **Conditional GET** — compute the strong ETag from the cached
//!    `target_hash` and raw query; return `304` if `If-None-Match` matches,
//!    else render and serve `200`.
//!
//! Freshness rests entirely on Control Plane invalidation, which requires both
//! planes to share one cache handle in one process (see [`crate::cache`]).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;

use axum::Router;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::cache::CachedSnippet;
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
    // database. The structural check (length, then charset) sheds malformed
    // junk before the id is materialized — and before the keyed-MAC verify
    // below — so it is the cheap pre-gate. Indistinguishable from "not found".
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

    // Immutable shortcut: for content-addressed ids the ETag is just `"<id>"`.
    // Build the string only when the client sent If-None-Match — skips one
    // format! per request without a validator (the common cold-cache first load)
    // and per mutable-route request where the immutable ETag is never relevant.
    // Carry the string forward into compute_etag to avoid a second format!.
    let immutable_etag = inm.map(|_| format!("\"{}\"", raw_id));
    if let Some(v) = inm
        && let Some(s) = immutable_etag.as_deref()
        && v.to_str().is_ok_and(|sv| sv.trim() == s)
    {
        return build_not_modified(str_to_header(s), cache_control_for(CacheMode::Immutable));
    }

    // Read-through the cache: a hit returns immediately; concurrent misses are
    // coalesced into a single 1-RTT load. A present entry is always current
    // because the Control Plane invalidates on every write.
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
        immutable_etag.as_deref(),
    );
    if let Some(v) = inm
        && etag_matches(v, &etag)
    {
        return build_not_modified(etag, cache_control_for(snippet.cache_mode));
    }
    build_ok(&snippet, filename, query, etag)
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

/// A newtype that lets an `Arc<str>` be wrapped in `bytes::Bytes::from_owner`,
/// giving a zero-copy `Bytes` view over content already on the heap.
struct ArcStr(Arc<str>);
impl AsRef<[u8]> for ArcStr {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// Build a `200 OK` response rendering the snippet against the query variables.
fn build_ok(
    snippet: &CachedSnippet,
    filename: Option<&str>,
    query: Option<&str>,
    etag: HeaderValue,
) -> Response {
    let variables = parse_query(query);
    // `render` returns `Cow::Borrowed` when no substitutions occur — the regex
    // engine makes zero heap allocations and the content is untouched.
    // We exploit that: the Borrowed path clones the Arc (two atomics, no memcpy)
    // and wraps it in `Bytes::from_owner`, giving a truly zero-copy response
    // body. The Owned path moves the rendered String into Bytes, also zero-copy.
    let body: Bytes = match renderer::render(&snippet.content, &variables) {
        Cow::Borrowed(_) => Bytes::from_owner(ArcStr(Arc::clone(&snippet.content))),
        Cow::Owned(s) => Bytes::from(s),
    };
    let content_type = resolve_content_type(filename, &snippet.content_type);
    let cc = cache_control_for(snippet.cache_mode);
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
    prebuilt_immutable: Option<&str>,
) -> HeaderValue {
    match mode {
        CacheMode::Immutable => match prebuilt_immutable {
            Some(s) => str_to_header(s),
            None => str_to_header(&format!("\"{}\"", raw_id)),
        },
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
/// value; percent-encoding is decoded. The result is a `HashMap` for O(1)
/// lookup in the renderer — a linear-scan structure would expose an
/// O(placeholders × params) work factor exploitable by an attacker who sends
/// arbitrarily many query parameters.
///
/// Keys and values that require no percent-decoding are stored as
/// `Cow::Borrowed` slices into the query string, avoiding a heap allocation
/// per parameter on the common clean-key path. Only percent-encoded
/// characters produce `Cow::Owned` strings.
fn parse_query(query: Option<&str>) -> HashMap<Cow<'_, str>, Cow<'_, str>> {
    let Some(query) = query else {
        return HashMap::new();
    };
    form_urlencoded::parse(query.as_bytes()).collect()
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
/// * **Mutable** routes → `no-cache`. A downstream cache may store the response
///   but MUST revalidate with the origin (via the strong `ETag`) before reuse,
///   because our in-process invalidation cannot reach the edge. The steady
///   state is a cheap conditional GET answered with `304` from the cache, so
///   there is zero edge-staleness window.
fn cache_control_for(mode: CacheMode) -> HeaderValue {
    match mode {
        CacheMode::Immutable => HeaderValue::from_static("public, max-age=31536000, immutable"),
        CacheMode::Mutable => HeaderValue::from_static("no-cache"),
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
        assert_eq!(vars.get("name").unwrap().as_ref(), "hello world");
        assert_eq!(
            vars.get("port").unwrap().as_ref(),
            "9090",
            "last value wins"
        );
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
            cache_control_for(CacheMode::Immutable).to_str().unwrap(),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            cache_control_for(CacheMode::Mutable).to_str().unwrap(),
            "no-cache"
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
