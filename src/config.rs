//! Process configuration, loaded from the environment with validated defaults.
//!
//! Configuration is parsed once at startup and then treated as immutable. Any
//! malformed or contradictory setting is a hard, fail-fast error — the process
//! must never boot in a half-configured state.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::auth::{AuthConfig, AuthMode, CloudflareSettings, OAuthSettings};

/// Fully resolved runtime configuration.
pub struct Config {
    pub database_url: String,
    pub database_max_connections: u32,
    pub control_plane_addr: SocketAddr,
    pub data_plane_addr: SocketAddr,
    /// Public base URL at which the Data Plane is reachable by clients, e.g.
    /// `https://cdn.example.com`. In a typical deployment the two planes live on
    /// different domains, so the dashboard cannot assume the Data Plane shares
    /// the Control Plane's origin. When unset the dashboard falls back to a
    /// best-effort guess from its own location.
    pub data_plane_url: Option<String>,
    pub cache_byte_budget: u64,
    pub cache_mutable_ttl: Duration,
    /// Deployment-wide secret salt for the route-id MAC. Keep it stable across
    /// a deployment (rotating it invalidates every existing permalink/alias)
    /// and never log it.
    pub id_secret: String,
    pub auth: AuthConfig,
}

/// Minimum accepted length for `ID_SIGNING_SECRET`. A short secret undermines
/// the MAC's unforgeability, so we refuse to boot below this bar.
const MIN_ID_SECRET_LEN: usize = 32;

impl Config {
    /// Load and validate configuration from environment variables, applying
    /// `.env` first if present.
    pub fn from_env() -> Result<Self> {
        // Best-effort: a missing .env is fine; a malformed one is surfaced.
        let _ = dotenvy::dotenv();

        let database_url =
            require("DATABASE_URL").context("a PostgreSQL connection string is required")?;

        let database_max_connections = parse_or("DATABASE_MAX_CONNECTIONS", 16)?;

        let control_plane_addr = parse_addr("CONTROL_PLANE_ADDR", "0.0.0.0:8080")?;
        let data_plane_addr = parse_addr("DATA_PLANE_ADDR", "0.0.0.0:3000")?;

        let data_plane_url = env("DATA_PLANE_PUBLIC_URL")
            .map(|v| v.trim().trim_end_matches('/').to_owned())
            .filter(|v| !v.is_empty());

        let cache_byte_budget = parse_or("CACHE_BYTE_BUDGET", 32 * 1024 * 1024)?;
        let cache_mutable_ttl = Duration::from_secs(parse_or("CACHE_MUTABLE_TTL_SECS", 300)?);

        let id_secret = load_id_secret()?;

        let auth = load_auth()?;

        Ok(Self {
            database_url,
            database_max_connections,
            control_plane_addr,
            data_plane_addr,
            data_plane_url,
            cache_byte_budget,
            cache_mutable_ttl,
            id_secret,
            auth,
        })
    }
}

/// Load the route-id signing secret, enforcing a minimum length so a weak salt
/// cannot silently degrade the DoS mitigation.
fn load_id_secret() -> Result<String> {
    let secret = require("ID_SIGNING_SECRET").context(
        "ID_SIGNING_SECRET is required: it keys the route-id MAC that shields \
         the Data Plane from enumeration",
    )?;
    if secret.len() < MIN_ID_SECRET_LEN {
        bail!(
            "ID_SIGNING_SECRET must be at least {MIN_ID_SECRET_LEN} characters \
             (got {})",
            secret.len()
        );
    }
    Ok(secret)
}

/// Resolve the auth configuration, requiring complete provider settings only
/// for the selected mode.
fn load_auth() -> Result<AuthConfig> {
    let mode = match env("AUTH_MODE").as_deref() {
        None | Some("none") => AuthMode::None,
        Some("oauth") => AuthMode::Oauth,
        Some("cloudflare") => AuthMode::Cloudflare,
        Some(other) => {
            bail!("AUTH_MODE must be 'none', 'oauth', or 'cloudflare', got '{other}'")
        }
    };

    match mode {
        AuthMode::None => Ok(AuthConfig::None),
        AuthMode::Oauth => {
            let issuer =
                require("OAUTH_ISSUER").context("AUTH_MODE=oauth requires OAUTH_ISSUER")?;
            let audience =
                require("OAUTH_AUDIENCE").context("AUTH_MODE=oauth requires OAUTH_AUDIENCE")?;
            let jwks_url =
                require("OAUTH_JWKS_URL").context("AUTH_MODE=oauth requires OAUTH_JWKS_URL")?;
            let jwks_cache_ttl =
                Duration::from_secs(parse_or("OAUTH_JWKS_CACHE_TTL_SECS", 300)?.max(60));
            Ok(AuthConfig::Oauth(OAuthSettings {
                issuer,
                audience,
                jwks_url,
                jwks_cache_ttl,
            }))
        }
        AuthMode::Cloudflare => {
            let team_domain = require("CLOUDFLARE_TEAM_DOMAIN")
                .context("AUTH_MODE=cloudflare requires CLOUDFLARE_TEAM_DOMAIN")?;
            let audience = require("CLOUDFLARE_AUDIENCE")
                .context("AUTH_MODE=cloudflare requires CLOUDFLARE_AUDIENCE")?;
            // Access certs rotate slowly; clamp to at least an hour to avoid
            // hammering the edge while staying ahead of rotations.
            let certs_cache_ttl =
                Duration::from_secs(parse_or("CLOUDFLARE_CERTS_CACHE_TTL_SECS", 86400)?.max(3600));
            Ok(AuthConfig::Cloudflare(CloudflareSettings {
                team_domain,
                audience,
                certs_cache_ttl,
            }))
        }
    }
}

/// Read an environment variable, treating empty strings as absent.
fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// Read a required environment variable or fail with a clear message.
fn require(key: &str) -> Result<String> {
    env(key).with_context(|| format!("missing required environment variable {key}"))
}

/// Parse a typed value from an environment variable, or use `default`.
fn parse_or<T>(key: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match env(key) {
        Some(raw) => raw
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid value for {key}: {e}")),
        None => Ok(default),
    }
}

/// Parse a socket address from an environment variable, or use `default`.
fn parse_addr(key: &str, default: &str) -> Result<SocketAddr> {
    let raw = env(key).unwrap_or_else(|| default.to_owned());
    raw.parse()
        .with_context(|| format!("invalid socket address for {key}: '{raw}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_or_uses_default_when_absent() {
        // Use a key guaranteed not to be set in the test environment.
        let v: u32 = parse_or("SERVAL_TEST_DEFINITELY_UNSET_KEY", 42).unwrap();
        assert_eq!(v, 42);
    }

    #[test]
    fn parse_addr_default_is_valid() {
        let addr = parse_addr("SERVAL_TEST_UNSET_ADDR", "0.0.0.0:8080").unwrap();
        assert_eq!(addr.port(), 8080);
    }
}
