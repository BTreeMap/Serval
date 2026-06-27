//! Cryptographic primitives for Serval's content-addressed routing.
//!
//! Two identifiers exist in the system, both rendered as 64-character URL-safe
//! Base64 (no padding):
//!
//! * **Alias ids** — 48 bytes of CSPRNG output, used for mutable routes.
//! * **Content hashes** — `SHA3-384(content)`, used both as the immutable
//!   `content_blocks.hash_id` and as the `route_id` of an immutable permalink.
//!
//! Both inputs are exactly 48 bytes, which Base64-encodes to exactly 64
//! characters — the invariant the Data Plane relies on when it rejects any
//! request whose `id.len() != 64`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha3::{Digest, Sha3_384};

/// The exact character length of every Serval route id and content hash.
pub const ID_LEN: usize = 64;

/// Number of CSPRNG bytes drawn for a mutable alias id.
const ALIAS_ENTROPY_BYTES: usize = 48;

/// Generate a fresh, unguessable alias id for a mutable route.
///
/// Draws [`ALIAS_ENTROPY_BYTES`] from the operating system CSPRNG and encodes
/// them as URL-safe Base64 without padding, yielding a [`ID_LEN`]-character id.
#[must_use]
pub fn generate_alias_id() -> String {
    let mut bytes = [0u8; ALIAS_ENTROPY_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Compute the content address of `content` as `SHA3-384` over its UTF-8 bytes,
/// encoded as URL-safe Base64 without padding.
///
/// This value is simultaneously the `content_blocks.hash_id` and the
/// `route_id` of an immutable permalink — identical content always yields the
/// identical id, independent of any extension or MIME type.
#[must_use]
pub fn hash_content(content: &str) -> String {
    let mut hasher = Sha3_384::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_ids_are_64_chars() {
        let id = generate_alias_id();
        assert_eq!(id.len(), ID_LEN, "alias id must be exactly 64 chars");
    }

    #[test]
    fn alias_ids_are_unique() {
        let a = generate_alias_id();
        let b = generate_alias_id();
        assert_ne!(a, b, "two CSPRNG aliases must not collide");
    }

    #[test]
    fn alias_ids_are_url_safe() {
        let id = generate_alias_id();
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "alias id must be URL-safe Base64 (no pad): {id}"
        );
    }

    #[test]
    fn content_hash_is_64_chars() {
        let hash = hash_content("hello world");
        assert_eq!(hash.len(), ID_LEN, "content hash must be exactly 64 chars");
    }

    #[test]
    fn content_hash_is_deterministic() {
        // Acceptance criterion: permalink purity — identical text, identical id.
        let a = hash_content("the same bytes");
        let b = hash_content("the same bytes");
        assert_eq!(a, b, "identical content must hash identically");
    }

    #[test]
    fn content_hash_differs_for_different_content() {
        assert_ne!(hash_content("a"), hash_content("b"));
    }

    #[test]
    fn content_hash_matches_known_vector() {
        // SHA3-384("") URL-safe Base64 (no pad). Guards against algorithm drift.
        let empty = hash_content("");
        assert_eq!(empty.len(), ID_LEN);
        assert_eq!(
            empty,
            "DGOnW4ReT30BEH2FLkwkhcUaUKqqlPxhmV5xu-6YOirDcTgxJkrbR_tr0eBY1fAE"
        );
    }
}
