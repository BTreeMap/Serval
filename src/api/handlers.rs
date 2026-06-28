//! Control Plane snippet handlers: create, update, inspect, and restore routes.
//!
//! Writes uphold the system invariants at the boundary: content blocks are
//! immutable and content-addressed, every snippet is an editable route that may
//! be repointed only by its owner (or an admin), and **every** write evicts the
//! affected id from the shared delivery cache so the next Data Plane read is
//! fresh.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode};
use serde::{Deserialize, Serialize};

use super::error::ApiError;
use super::extract::Caller;
use crate::db::CreateRoute;
use crate::db::models::{ContentHash, RouteAnnotations, RouteId};
use crate::state::ControlState;

const DEFAULT_CONTENT_TYPE: &str = "text/plain; charset=utf-8";
const MAX_CONTENT_TYPE_LEN: usize = 255;
const MAX_TITLE_LEN: usize = 255;
const MAX_DESCRIPTION_LEN: usize = 4096;

/// Request body for creating a snippet.
#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    /// Raw template text to store.
    pub content: String,
    /// Optional MIME type; defaults to `text/plain; charset=utf-8`.
    #[serde(default)]
    pub content_type: Option<String>,
    /// Optional human-readable title for the snippet.
    #[serde(default)]
    pub title: Option<String>,
    /// Optional human-readable description for the snippet.
    #[serde(default)]
    pub description: Option<String>,
}

/// Request body for updating a snippet.
///
/// A partial update: supply `content` to repoint the route at a new version,
/// `content_type` to change its stored presentation metadata, `title` and/or
/// `description` to update those annotations, or any combination. At least one
/// field must be present.
#[derive(Debug, Deserialize)]
pub struct UpdateRequest {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    /// Set to an empty string to clear the title.
    #[serde(default)]
    pub title: Option<String>,
    /// Set to an empty string to clear the description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Request body for restoring a snippet to one of its earlier versions.
#[derive(Debug, Deserialize)]
pub struct RestoreRequest {
    /// The content hash of the version to restore, taken from the ledger.
    pub target_hash: String,
}

/// Representation of a route returned to the dashboard.
#[derive(Debug, Serialize)]
pub struct SnippetResponse {
    pub id: String,
    #[serde(flatten)]
    pub annotations: RouteAnnotations,
    pub owner_id: Option<String>,
}
/// A compact route listing entry for the dashboard index.
#[derive(Debug, Serialize)]
pub struct SnippetSummary {
    pub id: String,
    #[serde(flatten)]
    pub annotations: RouteAnnotations,
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
    #[serde(flatten)]
    pub annotations: RouteAnnotations,
    pub owner_id: Option<String>,
    pub history_count: usize,
    pub history: Vec<HistoryItem>,
}

/// The content of one historical version, returned for previewing.
#[derive(Debug, Serialize)]
pub struct VersionContent {
    pub target_hash: String,
    pub content: String,
}

/// The current caller's identity and role, for the UI.
#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub user_id: String,
    pub is_admin: bool,
}

/// Public app-bootstrap metadata the dashboard needs before it can
/// authenticate: the active auth flow and where the Data Plane lives.
#[derive(Debug, Serialize)]
pub struct AuthInfoResponse {
    /// The active mode: `none`, `oauth`, or `cloudflare`.
    pub mode: &'static str,
    /// Public base URL of the Data Plane (e.g. `https://cdn.example.com`), or
    /// `null` when unconfigured so the dashboard guesses from its own origin.
    pub data_plane_url: Option<String>,
    /// Frontend-safe OAuth bootstrap settings for the browser-managed flow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthFrontendConfig>,
}

/// Public OAuth bootstrap settings consumed by the frontend.
#[derive(Debug, Serialize)]
pub struct OAuthFrontendConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub scopes: String,
    pub redirect_uri: String,
}

/// `GET /api/auth-info` — report the public bootstrap metadata. Unauthenticated:
/// the sign-in screen needs this *before* it can present credentials, and the
/// dashboard needs the Data Plane URL to build delivery links.
pub async fn auth_info(State(state): State<ControlState>) -> Json<AuthInfoResponse> {
    Json(AuthInfoResponse {
        mode: state.auth.mode().as_str(),
        data_plane_url: state.data_plane_url.as_deref().map(str::to_owned),
        oauth: state
            .auth
            .oauth_frontend()
            .map(|oauth| OAuthFrontendConfig {
                issuer_url: oauth.issuer_url.clone(),
                client_id: oauth.client_id.clone(),
                scopes: oauth.scopes.clone(),
                redirect_uri: oauth.redirect_uri.clone(),
            }),
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
            annotations: r.annotations,
            owner_id: r.owner_id,
            updated_at: r.updated_at,
        })
        .collect();
    Ok(Json(snippets))
}

/// `POST /api/snippets` — create a new editable snippet.
pub async fn create_snippet(
    State(state): State<ControlState>,
    caller: Caller,
    Json(req): Json<CreateRequest>,
) -> Result<(StatusCode, Json<SnippetResponse>), ApiError> {
    let content_type = normalize_content_type(req.content_type)?;
    let title = normalize_annotation(req.title, "title", MAX_TITLE_LEN)?;
    let description = normalize_annotation(req.description, "description", MAX_DESCRIPTION_LEN)?;

    // Mint the signed content id (the content-block key) and an unguessable,
    // random route id. The route is the user-facing, editable handle; the
    // content hash is an internal address for this exact version.
    let hash = ContentHash::from_signed(state.signer.content_id(&req.content));
    let id = RouteId::from_signed(state.signer.random_id());

    let inserted = state
        .repo
        .create_route(CreateRoute {
            id: id.clone(),
            hash: hash.clone(),
            content: &req.content,
            content_type: &content_type,
            title: title.as_deref(),
            description: description.as_deref(),
            owner_id: Some(&caller.user_id),
            editor_id: &caller.user_id,
        })
        .await?;

    if !inserted {
        // A random alias collided with an existing id — vanishingly unlikely.
        // Surface it so the client can simply retry.
        return Err(ApiError::Conflict(
            "alias id collision, please retry".to_owned(),
        ));
    }

    // Evict any stale cache entry so the next delivery read reflects current
    // state.
    state.cache.invalidate(&id).await;

    let owner_id = Some(caller.user_id);

    Ok((
        StatusCode::CREATED,
        Json(SnippetResponse {
            id: id.into_inner(),
            annotations: RouteAnnotations {
                content_type,
                title,
                description,
            },
            owner_id,
        }),
    ))
}

/// `PATCH /api/snippets/{id}` — repoint a snippet at new content and/or change
/// its stored presentation metadata. At least one field must be present.
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

    // A partial update must change something.
    if req.content.is_none()
        && req.content_type.is_none()
        && req.title.is_none()
        && req.description.is_none()
    {
        return Err(ApiError::BadRequest(
            "update must set at least one field".to_owned(),
        ));
    }

    // Repoint at new content first, appending one history row for the version.
    if let Some(content) = req.content.as_deref() {
        let updated = state
            .repo
            .update_route(
                &id,
                &ContentHash::from_signed(state.signer.content_id(content)),
                content,
                &caller.user_id,
            )
            .await?;
        if !updated {
            return Err(ApiError::NotFound);
        }
    }

    // Presentation metadata is not a content version, so changing it records no
    // history entry.
    let content_type = match req.content_type {
        Some(requested) => {
            let normalized = normalize_content_type(Some(requested))?;
            if !state.repo.set_content_type(&id, &normalized).await? {
                return Err(ApiError::NotFound);
            }
            normalized
        }
        None => meta.annotations.content_type,
    };

    let title = match req.title {
        Some(raw) => {
            let normalized = normalize_annotation(Some(raw), "title", MAX_TITLE_LEN)?;
            if !state.repo.set_title(&id, normalized.as_deref()).await? {
                return Err(ApiError::NotFound);
            }
            normalized
        }
        None => meta.annotations.title,
    };

    let description = match req.description {
        Some(raw) => {
            let normalized = normalize_annotation(Some(raw), "description", MAX_DESCRIPTION_LEN)?;
            if !state
                .repo
                .set_description(&id, normalized.as_deref())
                .await?
            {
                return Err(ApiError::NotFound);
            }
            normalized
        }
        None => meta.annotations.description,
    };

    // Cross-thread invalidation: the next Data Plane GET must see new content.
    state.cache.invalidate(&id).await;

    Ok(Json(SnippetResponse {
        id: id.into_inner(),
        annotations: RouteAnnotations {
            content_type,
            title,
            description,
        },
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
        annotations: meta.annotations,
        owner_id: meta.owner_id,
        history_count,
        history,
    }))
}

/// `GET /api/snippets/{id}/versions/{hash}` — return the content of one
/// historical version of a snippet for previewing. The hash must be a genuine
/// version of this route, so a caller cannot read arbitrary content.
pub async fn get_version(
    State(state): State<ControlState>,
    caller: Caller,
    Path((raw_id, raw_hash)): Path<(String, String)>,
) -> Result<Json<VersionContent>, ApiError> {
    let id = RouteId::parse(&raw_id).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let hash = ContentHash::parse(&raw_hash).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let meta = state
        .repo
        .fetch_route_meta(&id)
        .await?
        .ok_or(ApiError::NotFound)?;

    authorize_write(&caller, meta.owner_id.as_deref())?;

    let content = state
        .repo
        .fetch_version_content(&id, &hash)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(VersionContent {
        target_hash: hash.into_inner(),
        content,
    }))
}

/// `POST /api/snippets/{id}/restore` — repoint a snippet at one of its earlier
/// versions, recording the restore as a new entry in the ledger.
pub async fn restore_snippet(
    State(state): State<ControlState>,
    caller: Caller,
    Path(raw_id): Path<String>,
    Json(req): Json<RestoreRequest>,
) -> Result<Json<SnippetResponse>, ApiError> {
    let id = RouteId::parse(&raw_id).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let hash =
        ContentHash::parse(&req.target_hash).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let meta = state
        .repo
        .fetch_route_meta(&id)
        .await?
        .ok_or(ApiError::NotFound)?;

    authorize_write(&caller, meta.owner_id.as_deref())?;

    let restored = state
        .repo
        .restore_version(&id, &hash, &caller.user_id)
        .await?;
    if !restored {
        return Err(ApiError::BadRequest(
            "target_hash is not a version of this snippet".to_owned(),
        ));
    }

    // Cross-thread invalidation: the next Data Plane GET must see new content.
    state.cache.invalidate(&id).await;

    Ok(Json(SnippetResponse {
        id: id.into_inner(),
        annotations: meta.annotations,
        owner_id: meta.owner_id,
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

/// Validate and normalize an optional free-text annotation (title or
/// description). Trims whitespace; treats empty as absent (`None`). Returns
/// `Ok(None)` if the input was absent or became empty after trimming.
fn normalize_annotation(
    value: Option<String>,
    field: &str,
    max_len: usize,
) -> Result<Option<String>, ApiError> {
    match value {
        None => Ok(None),
        Some(v) => {
            let trimmed = v.trim().to_owned();
            if trimmed.is_empty() {
                Ok(None)
            } else if trimmed.len() > max_len {
                Err(ApiError::BadRequest(format!(
                    "{field} exceeds {max_len} characters"
                )))
            } else {
                Ok(Some(trimmed))
            }
        }
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
    use serde_json::json;

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

    #[test]
    fn auth_info_serializes_oauth_config_when_present() {
        let response = AuthInfoResponse {
            mode: "oauth",
            data_plane_url: Some("https://cdn.example.com".to_owned()),
            oauth: Some(OAuthFrontendConfig {
                issuer_url: "https://issuer.example.com".to_owned(),
                client_id: "serval-web".to_owned(),
                scopes: "openid profile email".to_owned(),
                redirect_uri: "https://app.example.com/auth/callback".to_owned(),
            }),
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "mode": "oauth",
                "data_plane_url": "https://cdn.example.com",
                "oauth": {
                    "issuer_url": "https://issuer.example.com",
                    "client_id": "serval-web",
                    "scopes": "openid profile email",
                    "redirect_uri": "https://app.example.com/auth/callback"
                }
            })
        );
    }

    #[test]
    fn auth_info_omits_oauth_config_when_absent() {
        let response = AuthInfoResponse {
            mode: "cloudflare",
            data_plane_url: None,
            oauth: None,
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "mode": "cloudflare",
                "data_plane_url": null
            })
        );
    }

    #[test]
    fn annotation_absent_yields_none() {
        assert_eq!(normalize_annotation(None, "title", 255).unwrap(), None);
    }

    #[test]
    fn annotation_empty_or_whitespace_yields_none() {
        assert_eq!(
            normalize_annotation(Some(String::new()), "title", 255).unwrap(),
            None
        );
        assert_eq!(
            normalize_annotation(Some("   ".to_owned()), "title", 255).unwrap(),
            None
        );
    }

    #[test]
    fn annotation_trims_whitespace() {
        assert_eq!(
            normalize_annotation(Some("  hello  ".to_owned()), "title", 255).unwrap(),
            Some("hello".to_owned())
        );
    }

    #[test]
    fn annotation_rejects_overlong() {
        let long = "a".repeat(MAX_TITLE_LEN + 1);
        assert!(matches!(
            normalize_annotation(Some(long), "title", MAX_TITLE_LEN),
            Err(ApiError::BadRequest(_))
        ));
    }
}
