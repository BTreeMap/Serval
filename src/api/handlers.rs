//! Control Plane snippet handlers: create, update, and inspect routes.
//!
//! Writes uphold the system invariants at the boundary: permalinks are derived
//! purely from content and are immutable, mutable aliases may be repointed only
//! by their owner (or an admin), and **every** write evicts the affected id
//! from the shared delivery cache so the next Data Plane read is fresh.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode};
use serde::{Deserialize, Serialize};

use super::error::ApiError;
use super::extract::Caller;
use crate::db::CreateRoute;
use crate::db::models::{CacheMode, ContentHash, RouteId};
use crate::state::ControlState;

const DEFAULT_CONTENT_TYPE: &str = "text/plain; charset=utf-8";
const MAX_CONTENT_TYPE_LEN: usize = 255;

/// Request body for creating a snippet.
#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    /// Raw template text to store.
    pub content: String,
    /// Optional MIME type; defaults to `text/plain; charset=utf-8`.
    #[serde(default)]
    pub content_type: Option<String>,
    /// When `true`, mint an immutable permalink (id = content hash) instead of
    /// a mutable alias.
    #[serde(default)]
    pub immutable: bool,
}

/// Request body for updating a mutable snippet.
#[derive(Debug, Deserialize)]
pub struct UpdateRequest {
    pub content: String,
}

/// Representation of a route returned to the dashboard.
#[derive(Debug, Serialize)]
pub struct SnippetResponse {
    pub id: String,
    pub immutable: bool,
    pub content_type: String,
    pub owner_id: Option<String>,
}
/// A compact route listing entry for the dashboard index.
#[derive(Debug, Serialize)]
pub struct SnippetSummary {
    pub id: String,
    pub immutable: bool,
    pub content_type: String,
    pub owner_id: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
/// One history ledger entry, serialized for the UI.
#[derive(Debug, Serialize)]
pub struct HistoryItem {
    pub target_hash: String,
    pub editor_id: String,
    pub changed_at: chrono::DateTime<chrono::Utc>,
}

/// Detailed view of a route, including its version ledger.
#[derive(Debug, Serialize)]
pub struct SnippetDetail {
    pub id: String,
    pub immutable: bool,
    pub content_type: String,
    pub owner_id: Option<String>,
    pub history_count: usize,
    pub history: Vec<HistoryItem>,
}

/// The current caller's identity and role, for the UI.
#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub user_id: String,
    pub is_admin: bool,
}

/// Public auth metadata so the dashboard can pick the right sign-in flow
/// without first authenticating.
#[derive(Debug, Serialize)]
pub struct AuthInfoResponse {
    /// The active mode: `none`, `oauth`, or `cloudflare`.
    pub mode: &'static str,
}

/// `GET /api/auth-info` — report the configured auth mode. Unauthenticated: the
/// sign-in screen needs this *before* it can present credentials.
pub async fn auth_info(State(state): State<ControlState>) -> Json<AuthInfoResponse> {
    Json(AuthInfoResponse {
        mode: state.auth.mode().as_str(),
    })
}

/// `GET /api/me` — report the authenticated caller.
pub async fn me(caller: Caller) -> Json<MeResponse> {
    Json(MeResponse {
        user_id: caller.user_id,
        is_admin: caller.is_admin,
    })
}

/// `GET /api/snippets` — list the caller's routes, newest first.
pub async fn list_snippets(
    State(state): State<ControlState>,
    caller: Caller,
) -> Result<Json<Vec<SnippetSummary>>, ApiError> {
    let routes = state.repo.list_routes_for_owner(&caller.user_id).await?;
    let snippets = routes
        .into_iter()
        .map(|r| SnippetSummary {
            id: r.id,
            immutable: r.cache_mode == CacheMode::Immutable,
            content_type: r.content_type,
            owner_id: r.owner_id,
            updated_at: r.updated_at,
        })
        .collect();
    Ok(Json(snippets))
}

/// `POST /api/snippets` — create a mutable alias or immutable permalink.
pub async fn create_snippet(
    State(state): State<ControlState>,
    caller: Caller,
    Json(req): Json<CreateRequest>,
) -> Result<(StatusCode, Json<SnippetResponse>), ApiError> {
    let content_type = normalize_content_type(req.content_type)?;

    // Mint the signed content id once: it is the content-block key and, for an
    // immutable permalink, the route id itself (one unified id format).
    let hash = ContentHash::from_signed(state.signer.content_id(&req.content));
    let (id, cache_mode) = if req.immutable {
        (
            RouteId::from_signed(hash.as_str().to_owned()),
            CacheMode::Immutable,
        )
    } else {
        (
            RouteId::from_signed(state.signer.random_id()),
            CacheMode::Mutable,
        )
    };

    let inserted = state
        .repo
        .create_route(CreateRoute {
            id: id.clone(),
            hash: hash.clone(),
            content: &req.content,
            content_type: &content_type,
            cache_mode,
            owner_id: Some(&caller.user_id),
            editor_id: &caller.user_id,
        })
        .await?;

    if !inserted && !req.immutable {
        // A random alias collided with an existing id — vanishingly unlikely.
        // Surface it so the client can simply retry.
        return Err(ApiError::Conflict(
            "alias id collision, please retry".to_owned(),
        ));
    }

    // Evict any stale cache entry (e.g. an identical permalink re-created after
    // an eviction) so the next delivery read reflects current state.
    state.cache.invalidate(&id).await;

    let owner_id = Some(caller.user_id);
    let status = if inserted {
        StatusCode::CREATED
    } else {
        // Idempotent permalink re-creation: the content already had this id.
        StatusCode::OK
    };

    Ok((
        status,
        Json(SnippetResponse {
            id: id.into_inner(),
            immutable: req.immutable,
            content_type,
            owner_id,
        }),
    ))
}

/// `PATCH /api/snippets/{id}` — repoint a mutable alias at new content.
pub async fn update_snippet(
    State(state): State<ControlState>,
    caller: Caller,
    Path(raw_id): Path<String>,
    Json(req): Json<UpdateRequest>,
) -> Result<Json<SnippetResponse>, ApiError> {
    let id = RouteId::parse(&raw_id).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let meta = state
        .repo
        .fetch_route_meta(&id)
        .await?
        .ok_or(ApiError::NotFound)?;

    authorize_write(&caller, meta.owner_id.as_deref())?;

    if meta.cache_mode == CacheMode::Immutable {
        return Err(ApiError::Conflict(
            "immutable permalinks cannot be updated".to_owned(),
        ));
    }

    let updated = state
        .repo
        .update_route(
            &id,
            &ContentHash::from_signed(state.signer.content_id(&req.content)),
            &req.content,
            &caller.user_id,
        )
        .await?;
    if !updated {
        return Err(ApiError::NotFound);
    }

    // Cross-thread invalidation: the next Data Plane GET must see new content.
    state.cache.invalidate(&id).await;

    Ok(Json(SnippetResponse {
        id: id.into_inner(),
        immutable: false,
        content_type: meta.content_type,
        owner_id: meta.owner_id,
    }))
}

/// `GET /api/snippets/{id}` — return route metadata and its version ledger.
pub async fn get_snippet(
    State(state): State<ControlState>,
    caller: Caller,
    Path(raw_id): Path<String>,
) -> Result<Json<SnippetDetail>, ApiError> {
    let id = RouteId::parse(&raw_id).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let meta = state
        .repo
        .fetch_route_meta(&id)
        .await?
        .ok_or(ApiError::NotFound)?;

    authorize_write(&caller, meta.owner_id.as_deref())?;

    let history = state.repo.list_history(&id).await?;
    let history_count = history.len();
    let history = history
        .into_iter()
        .map(|h| HistoryItem {
            target_hash: h.target_hash,
            editor_id: h.editor_id,
            changed_at: h.changed_at,
        })
        .collect();

    Ok(Json(SnippetDetail {
        id: id.into_inner(),
        immutable: meta.cache_mode == CacheMode::Immutable,
        content_type: meta.content_type,
        owner_id: meta.owner_id,
        history_count,
        history,
    }))
}

/// Permit a write/inspect only for the route owner or an admin.
fn authorize_write(caller: &Caller, owner_id: Option<&str>) -> Result<(), ApiError> {
    if caller.is_admin || owner_id == Some(caller.user_id.as_str()) {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// Validate and default the requested content type, ensuring it is a legal
/// HTTP header value the Data Plane can echo back.
fn normalize_content_type(requested: Option<String>) -> Result<String, ApiError> {
    let value = requested
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_CONTENT_TYPE.to_owned());

    if value.len() > MAX_CONTENT_TYPE_LEN {
        return Err(ApiError::BadRequest(format!(
            "content_type exceeds {MAX_CONTENT_TYPE_LEN} characters"
        )));
    }
    if HeaderValue::from_str(&value).is_err() {
        return Err(ApiError::BadRequest(
            "content_type is not a valid header value".to_owned(),
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_defaults_when_absent() {
        assert_eq!(normalize_content_type(None).unwrap(), DEFAULT_CONTENT_TYPE);
        assert_eq!(
            normalize_content_type(Some("   ".to_owned())).unwrap(),
            DEFAULT_CONTENT_TYPE
        );
    }

    #[test]
    fn content_type_rejects_overlong() {
        let long = "a".repeat(MAX_CONTENT_TYPE_LEN + 1);
        assert!(matches!(
            normalize_content_type(Some(long)),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn content_type_rejects_illegal_header() {
        assert!(matches!(
            normalize_content_type(Some("bad\nvalue".to_owned())),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn authorize_allows_owner_and_admin() {
        let owner = Caller {
            user_id: "u1".to_owned(),
            is_admin: false,
        };
        assert!(authorize_write(&owner, Some("u1")).is_ok());

        let admin = Caller {
            user_id: "root".to_owned(),
            is_admin: true,
        };
        assert!(authorize_write(&admin, Some("someone-else")).is_ok());
    }

    #[test]
    fn authorize_rejects_other_user() {
        let other = Caller {
            user_id: "u2".to_owned(),
            is_admin: false,
        };
        assert!(matches!(
            authorize_write(&other, Some("u1")),
            Err(ApiError::Forbidden)
        ));
    }
}
