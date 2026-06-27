//! Shared JWKS-backed JWT validation.
//!
//! Both authentication providers Serval understands — a generic OAuth issuer and
//! Cloudflare Access — present the same shape: a signed JWT whose key set is
//! published at an HTTPS endpoint and rotated over time. This module owns that
//! common machinery: a TTL-bounded JWKS cache, validation of signature, issuer,
//! audience, and expiry, and a single forced refresh when a token references an
//! unknown `kid` (the key-rotation path). A still-missing key after that refresh
//! is a hard error rather than an infinite refresh loop.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use jsonwebtoken::{DecodingKey, Validation, decode, decode_header};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// The validated subset of token claims Serval consumes. Signature, issuer,
/// audience, and expiry are checked during decode; `sub` is mandatory.
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
}

/// Validates JWTs against a cached JWKS for a fixed issuer and audience.
pub struct JwtValidator {
    issuer: String,
    audience: String,
    jwks_url: String,
    cache_ttl: Duration,
    client: Client,
    keys: RwLock<HashMap<String, Arc<DecodingKey>>>,
    last_refresh: RwLock<Option<Instant>>,
}

impl JwtValidator {
    /// Build the validator and prime its JWKS cache so the first real request
    /// does not pay refresh latency.
    pub async fn new(
        issuer: String,
        audience: String,
        jwks_url: String,
        cache_ttl: Duration,
    ) -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("serval-auth/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build HTTP client for JWT validation")?;

        let validator = Self {
            issuer,
            audience,
            jwks_url,
            cache_ttl,
            client,
            keys: RwLock::new(HashMap::new()),
            last_refresh: RwLock::new(None),
        };
        validator.refresh_keys().await?;
        Ok(validator)
    }

    /// Validate a token and return its claims, refreshing JWKS if necessary.
    pub async fn validate(&self, token: &str) -> Result<Claims> {
        let header = decode_header(token).context("failed to parse token header")?;
        let kid = header
            .kid
            .ok_or_else(|| anyhow!("token header is missing 'kid'"))?;

        let key = self.decoding_key(&kid).await?;

        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        // `exp` is required and checked by default in jsonwebtoken.

        let data = decode::<Claims>(token, key.as_ref(), &validation)
            .context("token failed signature, claim, or expiry validation")?;
        Ok(data.claims)
    }

    /// Resolve a decoding key by `kid`, refreshing once on a cache miss or when
    /// the TTL has elapsed.
    async fn decoding_key(&self, kid: &str) -> Result<Arc<DecodingKey>> {
        if self.cache_expired().await {
            debug!("JWKS cache expired; refreshing");
            self.refresh_keys().await?;
        }

        if let Some(key) = self.keys.read().await.get(kid) {
            return Ok(Arc::clone(key));
        }

        // Unknown kid: a key rotation likely occurred. Refresh once more.
        debug!(kid, "key id absent from JWKS cache; forcing refresh");
        self.refresh_keys().await?;
        self.keys
            .read()
            .await
            .get(kid)
            .cloned()
            .ok_or_else(|| anyhow!("no JWKS entry for key id '{kid}'"))
    }

    async fn cache_expired(&self) -> bool {
        match *self.last_refresh.read().await {
            Some(at) => at.elapsed() > self.cache_ttl,
            None => true,
        }
    }

    /// Fetch the JWKS and atomically replace the cached key set.
    async fn refresh_keys(&self) -> Result<()> {
        let jwks: JwkSet = self
            .client
            .get(&self.jwks_url)
            .send()
            .await
            .context("failed to request JWKS")?
            .error_for_status()
            .context("JWKS endpoint returned an error status")?
            .json()
            .await
            .context("failed to parse JWKS response")?;

        let mut new_keys: HashMap<String, Arc<DecodingKey>> = HashMap::new();
        for jwk in jwks.keys {
            let Some(kid) = jwk.kid.clone() else {
                warn!("skipping JWKS entry without 'kid'");
                continue;
            };
            match jwk.decoding_key() {
                Ok(key) => {
                    new_keys.insert(kid, Arc::new(key));
                }
                Err(e) => warn!(kid, error = %e, "skipping unusable JWKS entry"),
            }
        }

        if new_keys.is_empty() {
            bail!("JWKS response contained no usable keys");
        }

        *self.keys.write().await = new_keys;
        *self.last_refresh.write().await = Some(Instant::now());
        Ok(())
    }

    /// Test-only constructor that injects a decoding key directly, bypassing the
    /// network so token validation can be exercised offline.
    #[cfg(test)]
    fn with_static_key(issuer: String, audience: String, kid: &str, key: DecodingKey) -> Self {
        let mut keys = HashMap::new();
        keys.insert(kid.to_owned(), Arc::new(key));
        Self {
            issuer,
            audience,
            jwks_url: "https://unused.example/jwks".to_owned(),
            cache_ttl: Duration::from_secs(300),
            client: Client::new(),
            keys: RwLock::new(keys),
            last_refresh: RwLock::new(Some(Instant::now())),
        }
    }
}

#[derive(Debug, Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

/// A single JSON Web Key. Only the fields Serval supports are modeled.
#[derive(Debug, Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
    #[serde(default)]
    k: Option<String>,
}

impl Jwk {
    fn decoding_key(&self) -> Result<DecodingKey> {
        match self.kty.as_str() {
            "RSA" => {
                let n = self.n.as_deref().context("RSA key missing modulus 'n'")?;
                let e = self.e.as_deref().context("RSA key missing exponent 'e'")?;
                DecodingKey::from_rsa_components(n, e).context("invalid RSA JWKS components")
            }
            "oct" => {
                let k = self.k.as_deref().context("symmetric key missing 'k'")?;
                DecodingKey::from_base64_secret(k).context("invalid symmetric JWKS secret")
            }
            other => bail!("unsupported JWKS key type '{other}'"),
        }
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use serde_json::json;

    use super::*;

    const SECRET: &[u8] = b"serval-test-shared-secret";
    const KID: &str = "test-key";
    const ISSUER: &str = "https://issuer.example";
    const AUDIENCE: &str = "serval";

    fn validator() -> JwtValidator {
        let key = DecodingKey::from_secret(SECRET);
        JwtValidator::with_static_key(ISSUER.to_owned(), AUDIENCE.to_owned(), KID, key)
    }

    fn mint(claims: serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(KID.to_owned());
        encode(&header, &claims, &EncodingKey::from_secret(SECRET)).unwrap()
    }

    fn future_exp() -> i64 {
        (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp()
    }

    #[tokio::test]
    async fn accepts_valid_token() {
        let token = mint(json!({
            "sub": "user-123",
            "email": "u@example.com",
            "iss": ISSUER,
            "aud": AUDIENCE,
            "exp": future_exp(),
        }));
        let claims = validator().validate(&token).await.unwrap();
        assert_eq!(claims.sub, "user-123");
        assert_eq!(claims.email.as_deref(), Some("u@example.com"));
    }

    #[tokio::test]
    async fn rejects_expired_token() {
        let token = mint(json!({
            "sub": "user-123",
            "iss": ISSUER,
            "aud": AUDIENCE,
            "exp": (chrono::Utc::now() - chrono::Duration::hours(1)).timestamp(),
        }));
        assert!(validator().validate(&token).await.is_err());
    }

    #[tokio::test]
    async fn rejects_wrong_audience() {
        let token = mint(json!({
            "sub": "user-123",
            "iss": ISSUER,
            "aud": "some-other-service",
            "exp": future_exp(),
        }));
        assert!(validator().validate(&token).await.is_err());
    }

    #[tokio::test]
    async fn rejects_wrong_issuer() {
        let token = mint(json!({
            "sub": "user-123",
            "iss": "https://evil.example",
            "aud": AUDIENCE,
            "exp": future_exp(),
        }));
        assert!(validator().validate(&token).await.is_err());
    }

    #[tokio::test]
    async fn rejects_token_missing_exp() {
        let token = mint(json!({
            "sub": "user-123",
            "iss": ISSUER,
            "aud": AUDIENCE,
        }));
        assert!(
            validator().validate(&token).await.is_err(),
            "tokens without an expiry must be rejected"
        );
    }

    #[tokio::test]
    async fn rejects_unknown_kid_without_network() {
        // A token signed with a kid absent from the cache must fail closed
        // rather than attempt (and here, succeed at) anything.
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("rotated-away".to_owned());
        let token = encode(
            &header,
            &json!({
                "sub": "x",
                "iss": ISSUER,
                "aud": AUDIENCE,
                "exp": future_exp(),
            }),
            &EncodingKey::from_secret(SECRET),
        )
        .unwrap();
        // Refresh will fail (no server), surfacing as an error — never a panic.
        assert!(validator().validate(&token).await.is_err());
    }

    #[test]
    fn rsa_jwk_builds_decoding_key() {
        // Minimal well-formed RSA components (base64url) just need to parse.
        let n = URL_SAFE_NO_PAD.encode([0xC0u8; 256]);
        let jwk = Jwk {
            kid: Some("r1".to_owned()),
            kty: "RSA".to_owned(),
            n: Some(n),
            e: Some(URL_SAFE_NO_PAD.encode([0x01, 0x00, 0x01])),
            k: None,
        };
        assert!(jwk.decoding_key().is_ok());
    }

    #[test]
    fn unsupported_kty_is_rejected() {
        let jwk = Jwk {
            kid: Some("e1".to_owned()),
            kty: "EC".to_owned(),
            n: None,
            e: None,
            k: None,
        };
        assert!(jwk.decoding_key().is_err());
    }
}
