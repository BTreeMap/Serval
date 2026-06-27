//! Domain model: validated newtypes and row shapes for the CAS layer.
//!
//! Invalid states are pushed into the type system. A [`RouteId`] cannot be
//! constructed unless it is exactly [`crypto::ID_LEN`] URL-safe Base64
//! characters, so the Data Plane's `id.len() != 64` rejection is enforced once
//! at the boundary rather than re-checked everywhere downstream.

use crate::crypto::{self, ID_LEN};

/// A validated 64-character route id (mutable alias or immutable permalink).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteId(String);

/// Error returned when a candidate route id fails validation.
#[derive(Debug, thiserror::Error)]
pub enum RouteIdError {
    #[error("route id must be {ID_LEN} characters, got {0}")]
    WrongLength(usize),
    #[error("route id contains a non URL-safe-Base64 character")]
    InvalidCharacter,
}

impl RouteId {
    /// Parse and validate an untrusted id (e.g. from a request path).
    pub fn parse(raw: &str) -> Result<Self, RouteIdError> {
        if raw.len() != ID_LEN {
            return Err(RouteIdError::WrongLength(raw.len()));
        }
        if !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(RouteIdError::InvalidCharacter);
        }
        Ok(Self(raw.to_owned()))
    }

    /// Construct a fresh, random alias id. Always valid by construction.
    #[must_use]
    pub fn new_alias() -> Self {
        Self(crypto::generate_alias_id())
    }

    /// Borrow the id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the newtype, yielding the owned string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for RouteId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A validated content address: `Base64URL(SHA3-384(content))`.
///
/// Constructed only by hashing content, so it is always exactly [`ID_LEN`]
/// characters and is, by definition, the id of the corresponding immutable
/// permalink.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentHash(String);

impl ContentHash {
    /// Compute the content address of `content`.
    #[must_use]
    pub fn of(content: &str) -> Self {
        Self(crypto::hash_content(content))
    }

    /// Borrow the hash as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the newtype, yielding the owned string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }

    /// The immutable permalink route id for this content (identical value).
    #[must_use]
    pub fn to_route_id(&self) -> RouteId {
        RouteId(self.0.clone())
    }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Caching policy for a route, stored as a `SMALLINT` in `routes.cache_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// `0` — mutable alias: short TTL, must be evicted on update.
    Mutable,
    /// `1` — immutable permalink: safe for long-lived edge caching.
    Immutable,
}

/// Error returned when an out-of-range integer is read for a [`CacheMode`].
#[derive(Debug, thiserror::Error)]
#[error("invalid cache_mode value: {0}")]
pub struct CacheModeError(i16);

impl CacheMode {
    /// The on-disk `SMALLINT` representation.
    #[must_use]
    pub fn as_i16(self) -> i16 {
        match self {
            CacheMode::Mutable => 0,
            CacheMode::Immutable => 1,
        }
    }

    /// Parse the `SMALLINT` representation read back from PostgreSQL.
    pub fn from_i16(value: i16) -> Result<Self, CacheModeError> {
        match value {
            0 => Ok(CacheMode::Mutable),
            1 => Ok(CacheMode::Immutable),
            other => Err(CacheModeError(other)),
        }
    }
}

/// The columns needed to serve a delivery request, produced by the index join
/// of `routes` against `content_blocks`.
#[derive(Debug, Clone)]
pub struct DeliveryRecord {
    pub content: String,
    pub content_type: String,
    pub cache_mode: CacheMode,
}

/// A locally tracked authenticated user.
///
/// Under OAuth the identity provider is the source of truth for *who* a user
/// is, but it is not always the source of truth for *authorization*: many
/// providers cannot express an application-level "admin" role. Serval therefore
/// keeps its own user table, upserted on every login, with a locally
/// administered [`User::is_admin`] flag that an operator can toggle out of band
/// (e.g. via the CLI) independent of any provider claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// The provider subject (`sub`) or local identity. Matches `owner_id` /
    /// `editor_id` on the routing and history tables.
    pub id: String,
    /// Whether this user holds the application-level admin role.
    pub is_admin: bool,
    /// First time the user was seen by Serval.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Most recent login.
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_id_rejects_wrong_length() {
        assert!(matches!(
            RouteId::parse("too-short"),
            Err(RouteIdError::WrongLength(9))
        ));
    }

    #[test]
    fn route_id_rejects_bad_characters() {
        let bad = "!".repeat(ID_LEN);
        assert!(matches!(
            RouteId::parse(&bad),
            Err(RouteIdError::InvalidCharacter)
        ));
    }

    #[test]
    fn route_id_accepts_valid_alias() {
        let alias = RouteId::new_alias();
        let reparsed = RouteId::parse(alias.as_str()).expect("alias must reparse");
        assert_eq!(alias, reparsed);
    }

    #[test]
    fn permalink_id_equals_content_hash() {
        let hash = ContentHash::of("permalink content");
        let route = hash.to_route_id();
        assert_eq!(route.as_str(), hash.as_str());
    }

    #[test]
    fn cache_mode_roundtrips() {
        for mode in [CacheMode::Mutable, CacheMode::Immutable] {
            assert_eq!(CacheMode::from_i16(mode.as_i16()).unwrap(), mode);
        }
    }

    #[test]
    fn cache_mode_rejects_unknown() {
        assert!(CacheMode::from_i16(7).is_err());
    }
}
