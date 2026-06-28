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

use std::sync::Arc;

use bytes::Bytes;

use axum::Router;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::cache::CachedSnippet;
use crate::crypto::ID_LEN;
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
    // Compare in bytes, preserving the previous OWS tolerance without building
    // a temporary String on every conditional request.
    if let Some(v) = inm
        && immutable_etag_matches(v, raw_id)
    {
        return build_not_modified(
            immutable_etag(raw_id),
            cache_control_for(CacheMode::Immutable),
        );
    }

    // Read-through the cache: a hit returns immediately; concurrent misses are
    // coalesced into a single 1-RTT load. A present entry is always current
    // because the Control Plane invalidates on every write.
    let repo = state.repo.clone();
    // `RouteId` is `Copy`, so the loader closure captures its own inline copy
    // (a 64-byte stack memcpy) with no allocation or refcount.
    let load_id = id;
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
        return build_not_modified(etag, cache_control_for(snippet.cache_mode));
    }
    build_ok(snippet, filename, query, etag)
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

/// A newtype that lets an `Arc<CachedSnippet>` be wrapped in
/// `bytes::Bytes::from_owner`, giving a zero-copy `Bytes` view over content
/// already on the heap.
struct CachedSnippetBody(Arc<CachedSnippet>);
impl AsRef<[u8]> for CachedSnippetBody {
    fn as_ref(&self) -> &[u8] {
        self.0.content.as_bytes()
    }
}

/// Build a `200 OK` response rendering the snippet against the query variables.
fn build_ok(
    snippet: Arc<CachedSnippet>,
    filename: Option<&str>,
    query: Option<&str>,
    etag: HeaderValue,
) -> Response {
    let body: Bytes = match query {
        Some(query) if !query.is_empty() => match renderer::render_query(&snippet.content, query) {
            std::borrow::Cow::Borrowed(_) => {
                Bytes::from_owner(CachedSnippetBody(Arc::clone(&snippet)))
            }
            std::borrow::Cow::Owned(s) => Bytes::from(s),
        },
        _ => Bytes::from_owner(CachedSnippetBody(Arc::clone(&snippet))),
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
/// * **Mutable** routes: `signer.etag_bytes(target_hash, raw_query)` — a keyed hash
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
        CacheMode::Immutable => immutable_etag(raw_id),
        CacheMode::Mutable => mutable_etag(target_hash, raw_query, state),
    }
}

/// ETag for a mutable route given its `target_hash` and the raw query bytes.
fn mutable_etag(target_hash: &str, raw_query: &[u8], state: &DeliveryState) -> HeaderValue {
    let etag = state.signer.etag_bytes(target_hash, raw_query);
    header_from_owned_bytes(etag)
}

fn immutable_etag(raw_id: &str) -> HeaderValue {
    let mut etag = [0u8; ID_LEN + 2];
    etag[0] = b'"';
    etag[1..ID_LEN + 1].copy_from_slice(raw_id.as_bytes());
    etag[ID_LEN + 1] = b'"';
    header_from_owned_bytes(etag)
}

fn header_from_owned_bytes<const N: usize>(bytes: [u8; N]) -> HeaderValue {
    HeaderValue::from_maybe_shared(Bytes::from_owner(bytes))
        .unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn immutable_etag_matches(inm: &HeaderValue, raw_id: &str) -> bool {
    let bytes = trim_ows(inm.as_bytes());
    bytes.len() == ID_LEN + 2
        && bytes[0] == b'"'
        && bytes[ID_LEN + 1] == b'"'
        && &bytes[1..ID_LEN + 1] == raw_id.as_bytes()
}

fn trim_ows(mut bytes: &[u8]) -> &[u8] {
    while matches!(bytes.first(), Some(b' ' | b'\t')) {
        bytes = &bytes[1..];
    }
    while matches!(bytes.last(), Some(b' ' | b'\t')) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

/// Returns `true` if the client's `If-None-Match` value matches our ETag.
fn etag_matches(inm: &HeaderValue, etag: &HeaderValue) -> bool {
    inm.as_bytes() == etag.as_bytes()
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
fn resolve_content_type(filename: Option<&str>, stored: &HeaderValue) -> HeaderValue {
    if let Some(name) = filename
        && let Some(guess) = mime_guess::from_path(name).first()
        && let Ok(value) = HeaderValue::from_str(guess.as_ref())
    {
        return value;
    }
    stored.clone()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_prefers_filename_extension() {
        let stored = HeaderValue::from_static("text/plain; charset=utf-8");
        let ct = resolve_content_type(Some("config.json"), &stored);
        assert_eq!(ct.to_str().unwrap(), "application/json");
    }

    #[test]
    fn content_type_falls_back_to_stored() {
        let stored = HeaderValue::from_static("text/plain; charset=utf-8");
        let ct = resolve_content_type(None, &stored);
        assert_eq!(ct.to_str().unwrap(), "text/plain; charset=utf-8");

        // Unknown extension also falls back to the stored type.
        let stored = HeaderValue::from_static("text/markdown");
        let ct = resolve_content_type(Some("file.unknownext"), &stored);
        assert_eq!(ct.to_str().unwrap(), "text/markdown");
    }

    #[test]
    fn html_is_served_as_is() {
        // The blanket `nosniff` + `default-src 'none'; sandbox` headers render
        // any document inert, so the MIME type is no longer rewritten — HTML
        // passes through whether it comes from the filename or the stored type.
        let stored = HeaderValue::from_static("text/plain; charset=utf-8");
        let ct = resolve_content_type(Some("page.html"), &stored);
        assert_eq!(ct.to_str().unwrap(), "text/html");

        let stored = HeaderValue::from_static("text/html; charset=utf-8");
        let ct = resolve_content_type(None, &stored);
        assert_eq!(ct.to_str().unwrap(), "text/html; charset=utf-8");
    }

    #[test]
    fn non_html_types_are_preserved() {
        let stored = HeaderValue::from_static("text/plain");
        let ct = resolve_content_type(Some("data.json"), &stored);
        assert_eq!(ct.to_str().unwrap(), "application/json");
        let stored = HeaderValue::from_static("image/svg+xml");
        let ct = resolve_content_type(None, &stored);
        assert_eq!(ct.to_str().unwrap(), "image/svg+xml");
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
