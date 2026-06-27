//! Authentication for the Control Plane.
//!
//! Auth answers *who* the caller is — it deliberately does **not** answer
//! *whether they are an admin*. Authorization is local: the [`crate::db`] users
//! table is the source of truth for the admin role, because OAuth providers
//! cannot always express an application-level role. The Control Plane therefore
//! resolves `is_admin` against the database, never against a token claim.
//!
//! Two modes are supported:
//! * [`AuthMode::Oauth`] — validate a Bearer JWT against the provider's JWKS.
//! * [`AuthMode::None`] — a local/development bypass that yields a fixed
//!   superuser identity. Intended for tests and single-operator setups only.

mod oauth;

use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;

pub use oauth::{OAuthSettings, OAuthValidator};

/// Which authentication strategy the Control Plane enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// No authentication: every request is a local superuser. Dev/test only.
    None,
    /// Validate a Bearer JWT against the configured OAuth provider.
    Oauth,
}

/// Resolved configuration for the auth layer.
pub enum AuthConfig {
    None,
    Oauth(OAuthSettings),
}

/// An authenticated caller. Identity only — authorization is resolved
/// separately against the local users table.
#[derive(Debug, Clone)]
pub struct Identity {
    /// Stable subject id (`sub`), used as `owner_id` / `editor_id`.
    pub user_id: String,
    /// Email address, when the provider supplies one.
    pub email: Option<String>,
    /// When `true`, the caller bypasses the local admin check entirely. Set
    /// only by [`AuthMode::None`]; never granted by a token.
    pub dev_superuser: bool,
}

/// Failure modes when authenticating a request.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing Authorization header")]
    MissingAuthorization,
    #[error("malformed Authorization header")]
    MalformedAuthorization,
    #[error("token validation failed: {0}")]
    InvalidToken(String),
}

/// The runtime authentication service shared across Control Plane handlers.
///
/// A present validator means [`AuthMode::Oauth`]; its absence is
/// [`AuthMode::None`] (the local superuser bypass).
pub struct AuthService {
    validator: Option<OAuthValidator>,
}

impl AuthService {
    /// Construct the service from resolved configuration, priming the JWKS
    /// cache up front so the first real request does not pay refresh latency.
    pub async fn new(config: AuthConfig) -> anyhow::Result<Self> {
        let validator = match config {
            AuthConfig::None => None,
            AuthConfig::Oauth(settings) => Some(OAuthValidator::new(settings).await?),
        };
        Ok(Self { validator })
    }

    /// Whether this service performs real authentication.
    #[must_use]
    pub fn is_enforcing(&self) -> bool {
        self.validator.is_some()
    }

    /// Authenticate a request from its headers, returning the caller identity.
    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<Identity, AuthError> {
        match &self.validator {
            None => Ok(Identity {
                user_id: "local-dev".to_owned(),
                email: None,
                dev_superuser: true,
            }),
            Some(validator) => {
                let token = bearer_token(headers)?;
                let claims = validator
                    .validate(token)
                    .await
                    .map_err(|e| AuthError::InvalidToken(e.to_string()))?;
                Ok(Identity {
                    user_id: claims.sub,
                    email: claims.email,
                    dev_superuser: false,
                })
            }
        }
    }
}

/// Extract and validate the `Authorization: Bearer <token>` header shape.
fn bearer_token(headers: &HeaderMap) -> Result<&str, AuthError> {
    let value = headers
        .get(AUTHORIZATION)
        .ok_or(AuthError::MissingAuthorization)?
        .to_str()
        .map_err(|_| AuthError::MalformedAuthorization)?;
    value
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or(AuthError::MalformedAuthorization)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn none_mode_yields_dev_superuser() {
        let service = AuthService::new(AuthConfig::None).await.unwrap();
        assert!(!service.is_enforcing());
        let id = service.authenticate(&HeaderMap::new()).await.unwrap();
        assert_eq!(id.user_id, "local-dev");
        assert!(id.dev_superuser);
    }

    #[test]
    fn bearer_token_requires_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "token-without-bearer".parse().unwrap());
        assert!(matches!(
            bearer_token(&headers),
            Err(AuthError::MalformedAuthorization)
        ));
    }

    #[test]
    fn bearer_token_rejects_empty() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer    ".parse().unwrap());
        assert!(matches!(
            bearer_token(&headers),
            Err(AuthError::MalformedAuthorization)
        ));
    }

    #[test]
    fn bearer_token_missing_header() {
        assert!(matches!(
            bearer_token(&HeaderMap::new()),
            Err(AuthError::MissingAuthorization)
        ));
    }

    #[test]
    fn bearer_token_extracts_value() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer abc.def.ghi".parse().unwrap());
        assert_eq!(bearer_token(&headers).unwrap(), "abc.def.ghi");
    }
}
