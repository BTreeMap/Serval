//! Domain model: validated newtypes and row shapes for the CAS layer.
//!
//! Invalid states are pushed into the type system. A [`RouteId`] cannot be
//! constructed unless it is exactly [`crypto::ID_LEN`] URL-safe Base64
//! characters, so the Data Plane's `id.len() != 64` rejection is enforced once
//! at the boundary rather than re-checked everywhere downstream.
//!
//! Route ids are *minted and signed* by [`crypto::IdSigner`], not here: the
//! signer owns the 32-byte-prefix + 16-byte-MAC construction. This module only
//! validates the character shape; the MAC is verified at the delivery boundary.

use crate::crypto::ID_LEN;

/// A validated 64-character route id.
///
/// Every snippet is an editable route addressed by an unguessable, signed id.
/// A content hash shares this exact format (it is itself a valid, signed id),
/// so the Data Plane can address a specific stored version directly — but that
/// is an internal delivery detail, never a separate user-facing kind of route.
///
/// Backed by a fixed-capacity, inline [`ArrayString<ID_LEN>`] rather than a
/// heap `String` — the type *is* "a validated string of exactly [`ID_LEN`]
/// characters". This makes the id `Copy` and keeps it entirely off the
/// allocator on the Data Plane hot path: `parse` writes the bytes into the
/// stack buffer, the loader-closure copy and moka's internal key clone are
/// 64-byte `memcpy`s (no refcount, no allocation), and the cache stores the key
/// inline in its node instead of in a separate per-entry heap allocation.
/// `ArrayString` preserves its UTF-8 invariant internally, so [`as_str`] stays
/// a safe, zero-cost borrow with no `unsafe` and no re-validation.
///
/// [`ArrayString<ID_LEN>`]: arrayvec::ArrayString
/// [`as_str`]: Self::as_str
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RouteId(arrayvec::ArrayString<ID_LEN>);

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
    ///
    /// The structural checks (length, then charset) run first, so malformed
    /// input is rejected up front; a well-formed id is then copied into the
    /// inline buffer without ever touching the heap. The length check
    /// guarantees the id fits the fixed [`ID_LEN`] capacity exactly.
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
        // Infallible: `raw.len() == ID_LEN` equals the buffer capacity.
        Ok(Self(
            arrayvec::ArrayString::from(raw).expect("len == ID_LEN"),
        ))
    }

    /// Adopt a freshly-minted, already-signed id from [`crypto::IdSigner`].
    ///
    /// The signer guarantees a valid 64-char URL-safe id, so this skips
    /// re-validation. Use [`RouteId::parse`] for any untrusted input instead.
    #[must_use]
    pub fn from_signed(id: String) -> Self {
        debug_assert_eq!(id.len(), ID_LEN, "signer must emit {ID_LEN}-char ids");
        Self(arrayvec::ArrayString::from(&id).expect("signer emits ID_LEN-char ids"))
    }

    /// Borrow the id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Yield an owned string. Used only on cold Control Plane response paths,
    /// never on the Data Plane hot path.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0.as_str().to_owned()
    }
}

impl std::fmt::Display for RouteId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.as_str())
    }
}

/// A validated content address: `BLAKE3(content) || keyed-MAC`, URL-safe Base64
/// encoded to exactly [`ID_LEN`] characters.
///
/// This is the immutable `content_blocks.hash_id` — the CAS dedup key — and it
/// shares the one id format: it is itself a valid, MAC-signed route id. The
/// Data Plane uses that property to serve a specific stored version directly by
/// its hash (an internal "version permalink"). Minted by [`crypto::IdSigner`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentHash(String);

/// Error returned when a candidate content hash fails validation.
#[derive(Debug, thiserror::Error)]
pub enum ContentHashError {
    #[error("content hash must be {ID_LEN} characters, got {0}")]
    WrongLength(usize),
    #[error("content hash contains a non URL-safe-Base64 character")]
    InvalidCharacter,
}

impl ContentHash {
    /// Parse and validate an untrusted content hash (e.g. from a request path).
    pub fn parse(raw: &str) -> Result<Self, ContentHashError> {
        if raw.len() != ID_LEN {
            return Err(ContentHashError::WrongLength(raw.len()));
        }
        if !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(ContentHashError::InvalidCharacter);
        }
        Ok(Self(raw.to_owned()))
    }

    /// Adopt a freshly-minted content id from [`crypto::IdSigner::content_id`].
    ///
    /// The signer guarantees a valid 64-char id, so this skips re-validation.
    #[must_use]
    pub fn from_signed(id: String) -> Self {
        debug_assert_eq!(id.len(), ID_LEN, "signer must emit {ID_LEN}-char ids");
        Self(id)
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
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Edge-caching policy for a delivery response. Derived at delivery time from
/// *how* the id resolved — it is not stored.
///
/// A live route can be repointed by its owner, so it is cached briefly behind
/// explicit invalidation. A content-hash id addresses one immutable stored
/// version, so it is safe to cache forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// A live route: short TTL, evicted on update.
    Mutable,
    /// A content-addressed version: safe for long-lived edge caching.
    Immutable,
}

/// The columns needed to serve a delivery request. The `cache_mode` is set by
/// the resolution path (live route vs. direct content-hash), not read from a
/// column. The `target_hash` is the content block's hash: for a live route this
/// is `routes.target_hash`; for a content-addressed id it is the id itself.
#[derive(Debug, Clone)]
pub struct DeliveryRecord {
    pub content: String,
    pub content_type: String,
    pub cache_mode: CacheMode,
    pub target_hash: String,
}

/// The mutable, un-historied presentation annotations stored on a route: its
/// `content_type` fallback plus the optional human-readable title and
/// description. Grouped as one value so every metadata-bearing row shape
/// declares the set once instead of repeating the three fields.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RouteAnnotations {
    pub content_type: String,
    pub title: Option<String>,
    pub description: Option<String>,
}

/// Routing-layer metadata for a single route, without its content. Used by the
/// Control Plane to enforce ownership before a write.
#[derive(Debug, Clone)]
pub struct RouteMeta {
    pub target_hash: String,
    pub annotations: RouteAnnotations,
    pub owner_id: Option<String>,
}

/// One entry in the append-only `pointer_history` ledger.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// The ledger row's own primary key. Never shown to the client directly,
    /// but it is the tiebreaker in `changed_at DESC, id DESC` ordering and the
    /// keyset value a pagination cursor resumes from.
    pub id: i64,
    pub target_hash: String,
    pub editor_id: String,
    pub changed_at: chrono::DateTime<chrono::Utc>,
}

/// A compact route listing entry for the dashboard: enough to render a row and
/// link through to detail, without loading content or history.
#[derive(Debug, Clone)]
pub struct RouteSummary {
    pub id: String,
    pub annotations: RouteAnnotations,
    pub owner_id: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
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
    use crate::crypto;

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
    fn route_id_accepts_signed_id() {
        let signer = crypto::IdSigner::new("test-secret");
        let signed = signer.random_id();
        let reparsed = RouteId::parse(&signed).expect("signed id must reparse");
        assert_eq!(reparsed.as_str(), signed);
    }

    #[test]
    fn from_signed_adopts_minted_id() {
        let signer = crypto::IdSigner::new("test-secret");
        let signed = signer.content_id("some content");
        let id = RouteId::from_signed(signed.clone());
        assert_eq!(id.as_str(), signed);
    }

    #[test]
    fn content_hash_is_64_chars() {
        let signer = crypto::IdSigner::new("test-secret");
        let hash = ContentHash::from_signed(signer.content_id("some content"));
        assert_eq!(hash.as_str().len(), ID_LEN);
    }

    #[test]
    fn content_hash_parses_a_signed_id() {
        let signer = crypto::IdSigner::new("test-secret");
        let signed = signer.content_id("some content");
        let hash = ContentHash::parse(&signed).expect("signed content id must parse");
        assert_eq!(hash.as_str(), signed);
    }

    #[test]
    fn content_hash_rejects_wrong_length() {
        assert!(matches!(
            ContentHash::parse("too-short"),
            Err(ContentHashError::WrongLength(9))
        ));
    }

    #[test]
    fn content_id_is_a_valid_route_id() {
        // One id format: a content hash is itself a valid, signed route id, so
        // the Data Plane can address a stored version directly by its hash.
        let signer = crypto::IdSigner::new("test-secret");
        let content_id = signer.content_id("some content");
        let hash = ContentHash::from_signed(content_id.clone());
        let route = RouteId::from_signed(content_id);
        assert_eq!(route.as_str(), hash.as_str());
    }
}
