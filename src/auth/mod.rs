//! Authentication for the Control Plane.
//!
//! Auth answers *who* the caller is — it deliberately does **not** answer
//! *whether they are an admin*. Authorization is local: the [`crate::db`] users
//! table is the source of truth for the admin role, because identity providers
//! cannot always express an application-level role. The Control Plane therefore
//! resolves `is_admin` against the database, never against a token claim.
//!
//! Three modes are supported:
//! * [`AuthMode::Oauth`] — validate a Bearer JWT against the provider's JWKS.
//! * [`AuthMode::Cloudflare`] — validate the `Cf-Access-Jwt-Assertion` header
//!   that Cloudflare Access injects on every proxied request, against the team's
//!   published certs. The browser is authenticated transparently by the edge, so
//!   the dashboard needs no token-paste step.
//! * [`AuthMode::None`] — a local/development bypass that yields a fixed
//!   superuser identity. Intended for tests and single-operator setups only.

mod jwt;

use std::time::Duration;

use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;

use self::jwt::JwtValidator;

/// The HTTP header Cloudflare Access populates with the signed identity JWT.
const CF_ACCESS_HEADER: &str = "Cf-Access-Jwt-Assertion";

/// Which authentication strategy the Control Plane enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// No authentication: every request is a local superuser. Dev/test only.
    None,
    /// Validate a Bearer JWT against the configured OAuth provider.
    Oauth,
    /// Validate the `Cf-Access-Jwt-Assertion` header issued by Cloudflare Access.
    Cloudflare,
}

impl AuthMode {
    /// Stable lowercase label reported to the dashboard so it can choose the
    /// right sign-in affordance.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMode::None => "none",
            AuthMode::Oauth => "oauth",
            AuthMode::Cloudflare => "cloudflare",
        }
    }
}

/// Resolved configuration for the auth layer.
pub enum AuthConfig {
    None,
    Oauth(OAuthSettings),
    Cloudflare(CloudflareSettings),
}

/// Static configuration for validating a generic OAuth provider's JWTs.
#[derive(Debug, Clone)]
pub struct OAuthSettings {
    pub issuer: String,
    pub audience: String,
    pub jwks_url: String,
    pub jwks_cache_ttl: Duration,
}

/// Static configuration for validating Cloudflare Access JWTs.
///
/// The issuer and the certs endpoint are both derived from the team domain:
/// Access signs tokens with `iss = <team_domain>` and publishes its keys at
/// `<team_domain>/cdn-cgi/access/certs`.
#[derive(Debug, Clone)]
pub struct CloudflareSettings {
    /// The Zero Trust team domain, e.g. `https://your-team.cloudflareaccess.com`.
    pub team_domain: String,
    /// The Access application's AUD tag.
    pub audience: String,
    pub certs_cache_ttl: Duration,
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
    #[error("missing credentials")]
    MissingAuthorization,
    #[error("malformed credentials")]
    MalformedAuthorization,
    #[error("token validation failed: {0}")]
    InvalidToken(String),
}

/// Where the caller's JWT is carried on the request.
#[derive(Debug, Clone, Copy)]
enum TokenSource {
    /// `Authorization: Bearer <jwt>`.
    Bearer,
    /// `Cf-Access-Jwt-Assertion: <jwt>` (injected by Cloudflare Access).
    CloudflareAccess,
}

/// A configured JWT strategy: validate a token drawn from `source`.
struct JwtAuth {
    validator: JwtValidator,
    source: TokenSource,
}

/// The runtime authentication service shared across Control Plane handlers.
///
/// A present [`JwtAuth`] means real authentication ([`AuthMode::Oauth`] or
/// [`AuthMode::Cloudflare`]); its absence is [`AuthMode::None`], the local
/// superuser bypass.
pub struct AuthService {
    mode: AuthMode,
    jwt: Option<JwtAuth>,
}

impl AuthService {
    /// Construct the service from resolved configuration, priming the JWKS
    /// cache up front so the first real request does not pay refresh latency.
    pub async fn new(config: AuthConfig) -> anyhow::Result<Self> {
        let (mode, jwt) = match config {
            AuthConfig::None => (AuthMode::None, None),
            AuthConfig::Oauth(settings) => {
                let validator = JwtValidator::new(
                    settings.issuer,
                    settings.audience,
                    settings.jwks_url,
                    settings.jwks_cache_ttl,
                )
                .await?;
                (
                    AuthMode::Oauth,
                    Some(JwtAuth {
                        validator,
                        source: TokenSource::Bearer,
                    }),
                )
            }
            AuthConfig::Cloudflare(settings) => {
                let issuer = settings.team_domain.trim_end_matches('/').to_owned();
                let certs_url = format!("{issuer}/cdn-cgi/access/certs");
                let validator = JwtValidator::new(
                    issuer,
                    settings.audience,
                    certs_url,
                    settings.certs_cache_ttl,
                )
                .await?;
                (
                    AuthMode::Cloudflare,
                    Some(JwtAuth {
                        validator,
                        source: TokenSource::CloudflareAccess,
                    }),
                )
            }
        };
        Ok(Self { mode, jwt })
    }

    /// The configured mode, surfaced to the dashboard so it can present the
    /// correct sign-in experience.
    #[must_use]
    pub fn mode(&self) -> AuthMode {
        self.mode
    }

    /// Whether this service performs real authentication.
    #[must_use]
    pub fn is_enforcing(&self) -> bool {
        self.jwt.is_some()
    }

    /// Authenticate a request from its headers, returning the caller identity.
    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<Identity, AuthError> {
        match &self.jwt {
            None => Ok(Identity {
                user_id: "local-dev".to_owned(),
                email: None,
                dev_superuser: true,
            }),
            Some(JwtAuth { validator, source }) => {
                let token = extract_token(headers, *source)?;
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

/// Pull the JWT off the request according to the configured token source.
fn extract_token(headers: &HeaderMap, source: TokenSource) -> Result<&str, AuthError> {
    match source {
        TokenSource::Bearer => bearer_token(headers),
        TokenSource::CloudflareAccess => cloudflare_token(headers),
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

/// Extract the raw JWT from the Cloudflare Access assertion header.
fn cloudflare_token(headers: &HeaderMap) -> Result<&str, AuthError> {
    let value = headers
        .get(CF_ACCESS_HEADER)
        .ok_or(AuthError::MissingAuthorization)?
        .to_str()
        .map_err(|_| AuthError::MalformedAuthorization)?
        .trim();
    if value.is_empty() {
        return Err(AuthError::MalformedAuthorization);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn none_mode_yields_dev_superuser() {
        let service = AuthService::new(AuthConfig::None).await.unwrap();
        assert!(!service.is_enforcing());
        assert_eq!(service.mode(), AuthMode::None);
        let id = service.authenticate(&HeaderMap::new()).await.unwrap();
        assert_eq!(id.user_id, "local-dev");
        assert!(id.dev_superuser);
    }

    #[test]
    fn auth_mode_labels_are_stable() {
        assert_eq!(AuthMode::None.as_str(), "none");
        assert_eq!(AuthMode::Oauth.as_str(), "oauth");
        assert_eq!(AuthMode::Cloudflare.as_str(), "cloudflare");
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

    #[test]
    fn cloudflare_token_extracts_raw_jwt() {
        let mut headers = HeaderMap::new();
        headers.insert(CF_ACCESS_HEADER, "abc.def.ghi".parse().unwrap());
        assert_eq!(cloudflare_token(&headers).unwrap(), "abc.def.ghi");
    }

    #[test]
    fn cloudflare_token_missing_header() {
        assert!(matches!(
            cloudflare_token(&HeaderMap::new()),
            Err(AuthError::MissingAuthorization)
        ));
    }

    #[test]
    fn cloudflare_token_rejects_empty() {
        let mut headers = HeaderMap::new();
        headers.insert(CF_ACCESS_HEADER, "   ".parse().unwrap());
        assert!(matches!(
            cloudflare_token(&headers),
            Err(AuthError::MalformedAuthorization)
        ));
    }
}
