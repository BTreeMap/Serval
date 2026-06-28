//! Cryptographic primitives for Serval's content-addressed routing.
//!
//! ## One hash family: BLAKE3
//!
//! Every primitive here is **BLAKE3** — a modern, heavily-vetted cryptographic
//! hash with 128-bit collision and 256-bit (second-)preimage resistance, built
//! on the ChaCha permutation. It replaces both SHA-3 and HMAC.
//!
//! ## One id format
//!
//! There is a single 64-character id shape (48 raw bytes, URL-safe Base64, no
//! padding) — the invariant the Data Plane relies on when it rejects any
//! request whose `id.len() != 64`. Those 48 bytes always split into:
//!
//! * **32-byte prefix** — the *what*.
//! * **16-byte MAC** — the *proof*. `BLAKE3::keyed_hash(key, prefix)` truncated
//!   to 128 bits, where `key` is derived from a deployment-wide secret salt.
//!   Truncating a keyed hash does not weaken the surviving bits, so forging a
//!   tag still costs `2^128` work — infeasible in an online request scenario.
//!   The MAC is never stored; it is recomputed and checked on every read.
//!
//! Two kinds of id share that one format, differing only in how the prefix is
//! chosen:
//!
//! * **Content id** ([`IdSigner::content_id`]) — `prefix = BLAKE3(content)` (a
//!   full 32-byte digest). This is simultaneously the immutable
//!   `content_blocks.hash_id` (the CAS dedup key) and the route id of an
//!   immutable permalink. A content address is therefore itself a servable,
//!   MAC-valid route id.
//! * **Snippet id** ([`IdSigner::random_id`]) — `prefix =` 32 CSPRNG bytes, for
//!   a mutable alias.
//!
//! ## Why the MAC (DoS mitigation)
//!
//! Without it the id space is trivially enumerable: an attacker mints arbitrary
//! 64-char strings and hammers the Data Plane, forcing a cache miss and a
//! PostgreSQL `SELECT` per bogus id. The keyed MAC makes a valid id unforgeable
//! without the secret, so the Data Plane rejects forgeries with a single
//! constant-time check **before** touching the cache or the database,
//! collapsing the amplification vector.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use subtle::ConstantTimeEq;

/// The exact character length of every Serval route id and content hash.
pub const ID_LEN: usize = 64;

/// Bytes of the prefix embedded in every id. Matches BLAKE3's native 32-byte
/// digest, so a content prefix is exactly one full `BLAKE3(content)`.
const PREFIX_LEN: usize = 32;

/// Bytes of the keyed MAC appended to every id (BLAKE3 keyed hash truncated).
const MAC_LEN: usize = 16;

/// Total raw bytes of an id (`PREFIX_LEN + MAC_LEN`). Encodes to exactly
/// [`ID_LEN`] URL-safe Base64 characters with no padding.
const ID_BYTE_LEN: usize = PREFIX_LEN + MAC_LEN;

/// Domain-separation context for deriving the MAC key from the raw secret.
/// Bumping this string rotates every id deployment-wide.
const KEY_DERIVATION_CONTEXT: &str = "serval route-id mac v1";

/// Domain-separation context for deriving the ETag key. Kept distinct from
/// the MAC key so the permanent content-serving hash is never derivable from
/// an ETag value even if the etag key is somehow exposed.
const ETAG_KEY_DERIVATION_CONTEXT: &str = "serval delivery etag v1";

/// Mints and verifies signed ids.
///
/// Holds a 32-byte BLAKE3 key derived from the deployment secret. Cloning is
/// cheap (a fixed-size array copy); the key is never logged or `Debug`-printed.
#[derive(Clone)]
pub struct IdSigner {
    key: [u8; blake3::KEY_LEN],
    /// Separate key for ETag derivation so the permanent content-serving hash
    /// is never exposed through the ETag value.
    etag_key: [u8; blake3::KEY_LEN],
}

impl IdSigner {
    /// Derive a signer from the deployment secret salt.
    ///
    /// The raw secret is passed through BLAKE3's `derive_key` KDF with a fixed
    /// context string, so the actual 256-bit MAC key never equals the
    /// configured secret and rotating [`KEY_DERIVATION_CONTEXT`] invalidates
    /// every id deployment-wide.
    #[must_use]
    pub fn new(secret: &str) -> Self {
        let key = blake3::derive_key(KEY_DERIVATION_CONTEXT, secret.as_bytes());
        let etag_key = blake3::derive_key(ETAG_KEY_DERIVATION_CONTEXT, secret.as_bytes());
        Self { key, etag_key }
    }

    /// Keyed MAC over a 32-byte prefix, truncated to [`MAC_LEN`] bytes.
    ///
    /// BLAKE3's native keyed mode needs no HMAC wrapper — it is already
    /// length-extension resistant.
    fn mac(&self, prefix: &[u8]) -> [u8; MAC_LEN] {
        let tag = blake3::keyed_hash(&self.key, prefix);
        let mut mac = [0u8; MAC_LEN];
        mac.copy_from_slice(&tag.as_bytes()[..MAC_LEN]);
        mac
    }

    /// Assemble a signed id from a 32-byte prefix: `prefix || MAC(prefix)`.
    fn assemble(&self, prefix: [u8; PREFIX_LEN]) -> String {
        let mut bytes = [0u8; ID_BYTE_LEN];
        bytes[..PREFIX_LEN].copy_from_slice(&prefix);
        bytes[PREFIX_LEN..].copy_from_slice(&self.mac(&prefix));
        URL_SAFE_NO_PAD.encode(bytes)
    }

    /// Mint the content id for `content`: `BLAKE3(content) || MAC`.
    ///
    /// This single value is both the `content_blocks.hash_id` (the CAS dedup
    /// key) and a valid, signed route id, so the Data Plane can address one
    /// exact stored version directly by its hash. Identical content always
    /// yields the identical id under a fixed secret — a deterministic, internal
    /// version address bound to the deployment key.
    #[must_use]
    pub fn content_id(&self, content: &str) -> String {
        let digest = blake3::hash(content.as_bytes());
        self.assemble(*digest.as_bytes())
    }

    /// Mint a fresh, unguessable id for a new editable snippet from 32 CSPRNG
    /// bytes.
    #[must_use]
    pub fn random_id(&self) -> String {
        let mut prefix = [0u8; PREFIX_LEN];
        rand::thread_rng().fill_bytes(&mut prefix);
        self.assemble(prefix)
    }

    /// Compute a strong ETag value for a mutable route's current version.
    ///
    /// The tag is `base64url(BLAKE3::keyed_hash(etag_key, target_hash_bytes ||
    /// raw_query))` wrapped in RFC 9110 double-quotes, e.g. `"Fg3xkP9…"`. The
    /// full 256-bit keyed hash is used: unlike the route-id MAC (truncated to
    /// fit the fixed 64-char id format), an ETag is an opaque validator with no
    /// length constraint, so there is no reason to discard hash bits.
    /// The `etag_key` is derived under a context string distinct from the
    /// route-id MAC key, so the permanent content-serving hash is never exposed.
    ///
    /// For immutable (content-addressed) ids the ETag is `"<id>"` — computed
    /// directly by the delivery layer from the verified id, not through this
    /// method.
    #[must_use]
    pub fn etag(&self, target_hash: &str, raw_query: &[u8]) -> String {
        // Stream both inputs through the hasher instead of heap-allocating a
        // temporary Vec to concatenate them — equivalent output, one fewer alloc.
        let mut hasher = blake3::Hasher::new_keyed(&self.etag_key);
        hasher.update(target_hash.as_bytes());
        hasher.update(raw_query);
        let tag = hasher.finalize();
        let encoded = URL_SAFE_NO_PAD.encode(tag.as_bytes());
        format!("\"{}\"", encoded)
    }

    /// Verify that `id` carries a MAC this signer would produce.    ///
    /// Decodes the id, recomputes the MAC over its prefix, and compares in
    /// constant time. Returns `false` for any malformed, wrong-length, or
    /// forged id. This is the Data Plane's stateless admission gate.
    #[must_use]
    pub fn verify(&self, id: &str) -> bool {
        let Ok(bytes) = URL_SAFE_NO_PAD.decode(id) else {
            return false;
        };
        if bytes.len() != ID_BYTE_LEN {
            return false;
        }
        let (prefix, mac) = bytes.split_at(PREFIX_LEN);
        let expected = self.mac(prefix);
        expected.ct_eq(mac).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-deployment-secret-please-change";

    fn signer() -> IdSigner {
        IdSigner::new(SECRET)
    }

    fn is_url_safe(id: &str) -> bool {
        id.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    }

    #[test]
    fn content_id_is_64_url_safe_chars() {
        let id = signer().content_id("hello world");
        assert_eq!(id.len(), ID_LEN);
        assert!(is_url_safe(&id), "id must be URL-safe Base64: {id}");
    }

    #[test]
    fn random_id_is_64_url_safe_chars() {
        let id = signer().random_id();
        assert_eq!(id.len(), ID_LEN);
        assert!(is_url_safe(&id), "id must be URL-safe Base64: {id}");
    }

    #[test]
    fn content_id_is_deterministic_under_a_fixed_secret() {
        // Permalink purity: identical text, identical id.
        let s = signer();
        assert_eq!(s.content_id("same bytes"), s.content_id("same bytes"));
    }

    #[test]
    fn content_id_differs_for_different_content() {
        let s = signer();
        assert_ne!(s.content_id("a"), s.content_id("b"));
    }

    #[test]
    fn content_id_and_random_id_share_one_format() {
        // Both kinds of valid id are 64 URL-safe chars and verify under the
        // same signer — they differ only in how the 32-byte prefix is chosen.
        let s = signer();
        let content = s.content_id("payload");
        let random = s.random_id();
        assert_eq!(content.len(), ID_LEN);
        assert_eq!(random.len(), ID_LEN);
        assert!(is_url_safe(&content) && is_url_safe(&random));
        assert!(s.verify(&content) && s.verify(&random));
    }

    #[test]
    fn random_ids_are_unique() {
        let s = signer();
        assert_ne!(s.random_id(), s.random_id());
    }

    #[test]
    fn fresh_ids_verify() {
        let s = signer();
        assert!(s.verify(&s.content_id("verify me")));
        assert!(s.verify(&s.random_id()));
    }

    #[test]
    fn tampered_mac_is_rejected() {
        let s = signer();
        let id = s.random_id();
        // Flip the final character to corrupt the MAC region.
        let mut chars: Vec<char> = id.chars().collect();
        let last = chars.len() - 1;
        chars[last] = if chars[last] == 'A' { 'B' } else { 'A' };
        let forged: String = chars.into_iter().collect();
        assert_eq!(forged.len(), ID_LEN);
        assert!(!s.verify(&forged), "a tampered MAC must not verify");
    }

    #[test]
    fn tampered_prefix_is_rejected() {
        let s = signer();
        let id = s.content_id("original");
        // Flip a leading character to corrupt the content prefix.
        let mut chars: Vec<char> = id.chars().collect();
        chars[0] = if chars[0] == 'A' { 'B' } else { 'A' };
        let forged: String = chars.into_iter().collect();
        assert!(!s.verify(&forged), "a tampered prefix must not verify");
    }

    #[test]
    fn malformed_and_wrong_length_ids_are_rejected() {
        let s = signer();
        assert!(!s.verify("too-short"));
        assert!(!s.verify(&"A".repeat(ID_LEN + 4)));
        assert!(!s.verify("not base64 !!!!"));
    }

    #[test]
    fn ids_from_another_secret_are_rejected() {
        let mint = IdSigner::new("attacker-does-not-know-this");
        let guard = signer();
        assert!(!guard.verify(&mint.random_id()));
        assert!(!guard.verify(&mint.content_id("same content")));
    }

    #[test]
    fn key_derivation_separates_secrets() {
        // Distinct secrets must yield distinct verification behaviour.
        let a = IdSigner::new("secretA-aaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let b = IdSigner::new("secretB-bbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let id = a.random_id();
        assert!(a.verify(&id));
        assert!(!b.verify(&id));
    }

    #[test]
    fn etag_is_deterministic() {
        let s = signer();
        let hash = s.content_id("payload");
        assert_eq!(s.etag(&hash, b"port=8080"), s.etag(&hash, b"port=8080"));
    }

    #[test]
    fn etag_differs_by_query() {
        let s = signer();
        let hash = s.content_id("payload");
        assert_ne!(s.etag(&hash, b"port=8080"), s.etag(&hash, b"port=9090"));
    }

    #[test]
    fn etag_differs_by_hash() {
        let s = signer();
        let h1 = s.content_id("v1");
        let h2 = s.content_id("v2");
        assert_ne!(s.etag(&h1, b""), s.etag(&h2, b""));
    }

    #[test]
    fn etag_is_double_quoted_string() {
        let s = signer();
        let hash = s.content_id("payload");
        let tag = s.etag(&hash, b"");
        assert!(
            tag.starts_with('"') && tag.ends_with('"'),
            "ETag must be double-quoted: {tag}"
        );
    }

    #[test]
    fn etag_does_not_equal_content_id() {
        // The ETag key is distinct from the MAC key: the ETag value must not
        // equal the content id even when the query is empty.
        let s = signer();
        let hash = s.content_id("payload");
        let tag_inner = s.etag(&hash, b"").trim_matches('"').to_owned();
        assert_ne!(tag_inner, hash, "ETag inner must not equal the content id");
    }
}
