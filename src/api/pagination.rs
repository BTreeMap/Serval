//! Pagination primitives shared by every Control Plane collection endpoint.
//!
//! Every list is capped at [`MAX_PAGE_LIMIT`] rows and ordered by a stable,
//! database-side sort tuple (never `OFFSET`). A page's `next_cursor` is an
//! opaque, signed token that encodes exactly the keyset state needed to resume
//! the scan precisely where it left off: the last row's sort tuple, plus (for
//! history) the snapshot bookkeeping that keeps version numbers stable across
//! pages. The cursor is a MAC-protected pagination *position*, not a
//! capability — every query it drives still runs under the caller's existing
//! ownership check, exactly as the first page did.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use crate::crypto::{CURSOR_MAC_LEN, IdSigner};

use super::error::ApiError;

/// Hard cap on rows returned by a single collection request. Both endpoints
/// and the frontend must never request or emit more than this many rows in
/// one round trip.
pub const MAX_PAGE_LIMIT: u32 = 50;

/// A validated page size, `1..=MAX_PAGE_LIMIT`. Constructing one anywhere
/// guarantees the value already satisfies the API's page-size contract, so
/// call sites never re-check it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageLimit(u32);

impl PageLimit {
    /// Parse an optional `?limit=` query value. Absent defaults to
    /// [`MAX_PAGE_LIMIT`]; zero or values over the cap are rejected so a
    /// caller can never force an unbounded scan.
    pub fn parse(raw: Option<u32>) -> Result<Self, ApiError> {
        let limit = raw.unwrap_or(MAX_PAGE_LIMIT);
        if limit == 0 || limit > MAX_PAGE_LIMIT {
            return Err(ApiError::BadRequest(format!(
                "limit must be between 1 and {MAX_PAGE_LIMIT}, got {limit}"
            )));
        }
        Ok(Self(limit))
    }

    /// The validated value as `usize`, for slicing/allocating/binding.
    #[must_use]
    pub fn get(self) -> usize {
        self.0 as usize
    }

    /// The validated value as `i64`, for binding a `LIMIT` parameter.
    #[must_use]
    pub fn as_i64(self) -> i64 {
        i64::from(self.0)
    }

    /// The validated value as `u32`, for echoing back in a response envelope.
    #[must_use]
    pub fn as_u32(self) -> u32 {
        self.0
    }
}

/// Split a `limit + 1`-row fetch into the page to return and whether another
/// page follows — a fetch-one-extra trick that detects "has more" without a
/// second `COUNT` query.
#[must_use]
pub fn take_page<T>(mut rows: Vec<T>, limit: PageLimit) -> (Vec<T>, bool) {
    let has_more = rows.len() > limit.get();
    rows.truncate(limit.get());
    (rows, has_more)
}

/// The tagged union of every cursor this API mints. Tagging by `kind` makes a
/// cursor minted for one collection a type error (a signed `BadRequest`, not a
/// silent misinterpretation) if replayed against a different endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
enum CursorPayload {
    /// Resume position for the owner's snippet listing, ordered
    /// `updated_at DESC, id DESC`.
    #[serde(rename = "snippets_v1")]
    Snippets {
        updated_at: chrono::DateTime<chrono::Utc>,
        id: String,
    },
    /// Resume position for one route's version history, ordered
    /// `changed_at DESC, id DESC`. `snapshot_total` and `loaded_count` are
    /// carried forward from the first page so every page's `version_number`
    /// stays consistent even if the ledger grows while the caller is
    /// paginating through it.
    #[serde(rename = "history_v1")]
    History {
        changed_at: chrono::DateTime<chrono::Utc>,
        id: i64,
        snapshot_total: i64,
        loaded_count: i64,
    },
}

/// Encode a cursor payload as `base64url(json || mac)`.
fn encode(payload: &CursorPayload, signer: &IdSigner) -> String {
    let json = serde_json::to_vec(payload).expect("cursor payload always serializes");
    let mac = signer.sign_cursor(&json);
    let mut bytes = Vec::with_capacity(json.len() + mac.len());
    bytes.extend_from_slice(&json);
    bytes.extend_from_slice(&mac);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode and verify a cursor payload. Malformed base64, a truncated body, a
/// failed MAC, or invalid JSON all collapse to the same opaque `BadRequest` —
/// the client never learns which check failed.
fn decode(raw: &str, signer: &IdSigner) -> Result<CursorPayload, ApiError> {
    const INVALID: &str = "invalid or expired pagination cursor";

    let bytes = URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| ApiError::BadRequest(INVALID.to_owned()))?;
    if bytes.len() <= CURSOR_MAC_LEN {
        return Err(ApiError::BadRequest(INVALID.to_owned()));
    }
    let (json, mac) = bytes.split_at(bytes.len() - CURSOR_MAC_LEN);
    if !signer.verify_cursor(json, mac) {
        return Err(ApiError::BadRequest(INVALID.to_owned()));
    }
    serde_json::from_slice(json).map_err(|_| ApiError::BadRequest(INVALID.to_owned()))
}

/// Keyset cursor for `GET /api/snippets`, resuming a scan ordered
/// `updated_at DESC, id DESC`.
#[derive(Debug, Clone)]
pub struct SnippetsCursor {
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub id: String,
}

impl SnippetsCursor {
    #[must_use]
    pub fn encode(&self, signer: &IdSigner) -> String {
        encode(
            &CursorPayload::Snippets {
                updated_at: self.updated_at,
                id: self.id.clone(),
            },
            signer,
        )
    }

    /// Parse a caller-supplied `?cursor=` value, when present.
    pub fn parse(raw: Option<&str>, signer: &IdSigner) -> Result<Option<Self>, ApiError> {
        let Some(raw) = raw else {
            return Ok(None);
        };
        match decode(raw, signer)? {
            CursorPayload::Snippets { updated_at, id } => Ok(Some(Self { updated_at, id })),
            CursorPayload::History { .. } => Err(ApiError::BadRequest(
                "cursor was not minted for this endpoint".to_owned(),
            )),
        }
    }
}

/// Keyset cursor for a route's version-history page, resuming a scan ordered
/// `changed_at DESC, id DESC`, plus the version-numbering snapshot.
#[derive(Debug, Clone, Copy)]
pub struct HistoryCursor {
    pub changed_at: chrono::DateTime<chrono::Utc>,
    pub id: i64,
    pub snapshot_total: i64,
    pub loaded_count: i64,
}

impl HistoryCursor {
    #[must_use]
    pub fn encode(&self, signer: &IdSigner) -> String {
        encode(
            &CursorPayload::History {
                changed_at: self.changed_at,
                id: self.id,
                snapshot_total: self.snapshot_total,
                loaded_count: self.loaded_count,
            },
            signer,
        )
    }

    /// Parse a caller-supplied `?cursor=` value, when present.
    pub fn parse(raw: Option<&str>, signer: &IdSigner) -> Result<Option<Self>, ApiError> {
        let Some(raw) = raw else {
            return Ok(None);
        };
        match decode(raw, signer)? {
            CursorPayload::History {
                changed_at,
                id,
                snapshot_total,
                loaded_count,
            } => Ok(Some(Self {
                changed_at,
                id,
                snapshot_total,
                loaded_count,
            })),
            CursorPayload::Snippets { .. } => Err(ApiError::BadRequest(
                "cursor was not minted for this endpoint".to_owned(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-deployment-secret-please-change";

    fn signer() -> IdSigner {
        IdSigner::new(SECRET)
    }

    #[test]
    fn page_limit_defaults_to_max() {
        let limit = PageLimit::parse(None).expect("default limit");
        assert_eq!(limit.get(), MAX_PAGE_LIMIT as usize);
    }

    #[test]
    fn page_limit_accepts_in_range_value() {
        let limit = PageLimit::parse(Some(10)).expect("valid limit");
        assert_eq!(limit.get(), 10);
    }

    #[test]
    fn page_limit_rejects_zero() {
        assert!(PageLimit::parse(Some(0)).is_err());
    }

    #[test]
    fn page_limit_rejects_over_cap() {
        assert!(PageLimit::parse(Some(MAX_PAGE_LIMIT + 1)).is_err());
    }

    #[test]
    fn take_page_reports_has_more_from_extra_row() {
        let limit = PageLimit::parse(Some(2)).expect("valid limit");
        let (page, has_more) = take_page(vec![1, 2, 3], limit);
        assert_eq!(page, vec![1, 2]);
        assert!(has_more);
    }

    #[test]
    fn take_page_reports_no_more_when_exact() {
        let limit = PageLimit::parse(Some(3)).expect("valid limit");
        let (page, has_more) = take_page(vec![1, 2, 3], limit);
        assert_eq!(page, vec![1, 2, 3]);
        assert!(!has_more);
    }

    #[test]
    fn snippets_cursor_round_trips() {
        let s = signer();
        let cursor = SnippetsCursor {
            updated_at: chrono::Utc::now(),
            id: "route-id".to_owned(),
        };
        let token = cursor.encode(&s);
        let parsed = SnippetsCursor::parse(Some(&token), &s)
            .expect("parse")
            .expect("present");
        assert_eq!(parsed.id, cursor.id);
        assert_eq!(parsed.updated_at, cursor.updated_at);
    }

    #[test]
    fn snippets_cursor_absent_is_none() {
        let s = signer();
        assert!(SnippetsCursor::parse(None, &s).expect("parse").is_none());
    }

    #[test]
    fn history_cursor_round_trips() {
        let s = signer();
        let cursor = HistoryCursor {
            changed_at: chrono::Utc::now(),
            id: 42,
            snapshot_total: 101,
            loaded_count: 50,
        };
        let token = cursor.encode(&s);
        let parsed = HistoryCursor::parse(Some(&token), &s)
            .expect("parse")
            .expect("present");
        assert_eq!(parsed.id, cursor.id);
        assert_eq!(parsed.snapshot_total, cursor.snapshot_total);
        assert_eq!(parsed.loaded_count, cursor.loaded_count);
    }

    #[test]
    fn cursor_kind_mismatch_is_rejected() {
        let s = signer();
        let history_token = HistoryCursor {
            changed_at: chrono::Utc::now(),
            id: 1,
            snapshot_total: 1,
            loaded_count: 0,
        }
        .encode(&s);
        assert!(SnippetsCursor::parse(Some(&history_token), &s).is_err());
    }

    #[test]
    fn tampered_cursor_is_rejected() {
        let s = signer();
        let token = SnippetsCursor {
            updated_at: chrono::Utc::now(),
            id: "route-id".to_owned(),
        }
        .encode(&s);
        let mut tampered = token.clone();
        // Flip a character well inside the payload/MAC body.
        let mid = tampered.len() / 2;
        let mut chars: Vec<char> = tampered.chars().collect();
        chars[mid] = if chars[mid] == 'A' { 'B' } else { 'A' };
        tampered = chars.into_iter().collect();
        assert_ne!(tampered, token);
        assert!(SnippetsCursor::parse(Some(&tampered), &s).is_err());
    }

    #[test]
    fn cursor_from_another_secret_is_rejected() {
        let mint = IdSigner::new("attacker-does-not-know-this");
        let guard = signer();
        let token = SnippetsCursor {
            updated_at: chrono::Utc::now(),
            id: "route-id".to_owned(),
        }
        .encode(&mint);
        assert!(SnippetsCursor::parse(Some(&token), &guard).is_err());
    }

    #[test]
    fn malformed_cursor_is_rejected() {
        let s = signer();
        assert!(SnippetsCursor::parse(Some("not valid base64 !!!"), &s).is_err());
        assert!(SnippetsCursor::parse(Some(""), &s).is_err());
    }
}
