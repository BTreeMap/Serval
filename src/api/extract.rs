//! The authenticated-caller extractor for Control Plane handlers.
//!
//! Extracting a [`Caller`] performs the full Control Plane authentication
//! handshake: validate the request, record the user (refreshing `last_seen`),
//! and resolve the admin role **from the local users table** — never from a
//! token claim. The Data Plane never uses this; it is unauthenticated.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use super::error::ApiError;
use crate::auth::AuthError;
use crate::state::ControlState;

/// An authenticated caller with locally resolved authorization.
#[derive(Debug, Clone)]
pub struct Caller {
    pub user_id: String,
    pub is_admin: bool,
}

impl FromRequestParts<ControlState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ControlState,
    ) -> Result<Self, Self::Rejection> {
        let identity = state
            .auth
            .authenticate(&parts.headers)
            .await
            .map_err(|e| match e {
                AuthError::MissingAuthorization | AuthError::MalformedAuthorization => {
                    ApiError::Unauthorized
                }
                AuthError::InvalidToken(_) => ApiError::Unauthorized,
            })?;

        // Record the login / refresh last-seen. The Control Plane is a
        // low-traffic management surface, so a write per request is acceptable
        // and keeps the user ledger current.
        state.repo.upsert_user(&identity.user_id).await?;

        let is_admin = identity.dev_superuser || state.repo.is_admin(&identity.user_id).await?;

        Ok(Caller {
            user_id: identity.user_id,
            is_admin,
        })
    }
}
