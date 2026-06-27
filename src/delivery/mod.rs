//! The Data Plane: public, extreme-throughput snippet delivery.
//!
//! The hot path is GET-only and deliberately minimal: validate the id, read
//! through the byte-bounded cache (loading via the index join on a miss),
//! render the template against the query string, and return the bytes with a
//! cache policy derived from the route's mutability.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::cache::CachedSnippet;
use crate::db::models::{CacheMode, RouteId};
use crate::renderer;
use crate::state::DeliveryState;

/// Build the Data Plane router. Two shapes resolve the same route: a bare id,
/// and an id followed by a cosmetic filename whose extension drives the MIME
/// type. The id itself is never affected by the filename — permalink purity.
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
) -> Response {
    deliver(&state, &id, None, query.as_deref()).await
}

async fn deliver_named(
    State(state): State<DeliveryState>,
    Path((id, filename)): Path<(String, String)>,
    RawQuery(query): RawQuery,
) -> Response {
    deliver(&state, &id, Some(&filename), query.as_deref()).await
}

/// Shared delivery logic for both route shapes.
async fn deliver(
    state: &DeliveryState,
    raw_id: &str,
    filename: Option<&str>,
    query: Option<&str>,
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

    let repo = state.repo.clone();
    let load_id = id.clone();
    let loaded = state
        .cache
        .get_or_load(&id, move || async move {
            match repo.fetch_delivery(&load_id).await {
                Ok(Some(record)) => Ok(CachedSnippet::from(record)),
                Ok(None) => Err(LoadError::NotFound),
                Err(e) => Err(LoadError::Database(e)),
            }
        })
        .await;

    let snippet = match loaded {
        Ok(snippet) => snippet,
        Err(err) => return load_error_response(&err),
    };

    let variables = parse_query(query);
    let body = renderer::render(&snippet.content, &variables);

    let content_type = resolve_content_type(filename, &snippet.content_type);
    let cache_control = cache_control_for(snippet.cache_mode);

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache_control),
        ],
        body,
    )
        .into_response()
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
/// to the stored `content_type`. Falls back further to the stored value if the
/// guess is not a legal header.
///
/// The public plane never serves active HTML: because the query string is
/// reflected into the body unescaped and the client controls the filename
/// extension, honoring `text/html` would turn any snippet with a placeholder
/// into a reflected-XSS vector. Any `text/html` result — however it was derived
/// — is therefore downgraded to inert `text/plain; charset=utf-8`.
fn resolve_content_type(filename: Option<&str>, stored: &str) -> HeaderValue {
    if let Some(name) = filename
        && let Some(guess) = mime_guess::from_path(name).first()
        && let Ok(value) = HeaderValue::from_str(guess.as_ref())
    {
        return neutralize_html(value);
    }
    let value = HeaderValue::from_str(stored)
        .unwrap_or_else(|_| HeaderValue::from_static("text/plain; charset=utf-8"));
    neutralize_html(value)
}

/// Downgrade any `text/html` content type to inert `text/plain; charset=utf-8`,
/// leaving every other type untouched. The check ignores parameters and case so
/// `Text/HTML; charset=...` is caught as well.
fn neutralize_html(value: HeaderValue) -> HeaderValue {
    let is_html = value
        .to_str()
        .ok()
        .and_then(|s| s.split(';').next())
        .map(str::trim)
        .is_some_and(|essence| essence.eq_ignore_ascii_case("text/html"));

    if is_html {
        HeaderValue::from_static("text/plain; charset=utf-8")
    } else {
        value
    }
}

/// Choose a `Cache-Control` policy from the route's mutability. Immutable,
/// content-addressed routes are safe to cache aggressively at the edge; mutable
/// aliases get a short, revalidated TTL behind the in-process cache.
fn cache_control_for(mode: CacheMode) -> HeaderValue {
    match mode {
        CacheMode::Immutable => HeaderValue::from_static("public, max-age=31536000, immutable"),
        CacheMode::Mutable => HeaderValue::from_static("public, max-age=60, must-revalidate"),
    }
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
    fn html_from_filename_extension_is_downgraded() {
        // A client-chosen .html extension must never yield an active document.
        let ct = resolve_content_type(Some("page.html"), "text/plain; charset=utf-8");
        assert_eq!(ct.to_str().unwrap(), "text/plain; charset=utf-8");
    }

    #[test]
    fn html_from_stored_type_is_downgraded() {
        // Even a snippet stored as HTML is served inert, parameters and all.
        let ct = resolve_content_type(None, "text/html; charset=utf-8");
        assert_eq!(ct.to_str().unwrap(), "text/plain; charset=utf-8");

        let ct = resolve_content_type(None, "text/html");
        assert_eq!(ct.to_str().unwrap(), "text/plain; charset=utf-8");
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
            "public, max-age=60, must-revalidate"
        );
    }
}
