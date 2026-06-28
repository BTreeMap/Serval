//! The Data Plane delivery cache.
//!
//! # Why this differs from a naive entry-count cache
//!
//! Serval's payloads are large and highly variable (≈20 KiB average, often
//! more). Bounding the cache by *entry count* — as is common for small, uniform
//! values — would let a handful of large snippets blow the resident set far
//! past its intended budget. We therefore bound by **total weight in bytes**
//! via a [`weigher`], so memory stays predictable regardless of payload size.
//!
//! # Invalidation is the only freshness mechanism
//!
//! The Control Plane is the **sole writer** of `routes` and `content_blocks`,
//! and `content_blocks` are immutable. Both planes share **one** cache handle in
//! **one process**, so every write invalidates exactly the entry a read would
//! observe. There is no second writer and no clock-driven divergence, so a
//! cached entry is provably current between invalidations. This removes any need
//! for TTLs, staleness windows, or background refresh: an entry leaves the cache
//! only by explicit [`DeliveryCache::invalidate`] or byte-budget pressure.
//!
//! Two properties keep the hot path cheap under read spikes:
//!
//! * **Pointer-sized reads.** The value is an [`Arc<CachedSnippet>`] whose
//!   stored content type is a prevalidated header value. A cache hit clones
//!   handles, never the 20 KiB blob.
//! * **Stampede coalescing.** [`Cache::get_with`] collapses a thundering herd of
//!   concurrent misses for the same id into a single database load.
//!
//! [`weigher`]: moka::future::CacheBuilder::weigher

use std::future::Future;
use std::sync::Arc;

use axum::http::HeaderValue;
use moka::future::Cache;

use crate::db::models::{CacheMode, DeliveryRecord, RouteId};

/// An immutable, shareable snapshot of everything needed to serve a route.
///
/// The cache stores this behind an `Arc`; cloning the cache entry never copies
/// the content bytes. Rendering happens per request against the borrowed
/// `content`.
#[derive(Debug)]
pub struct CachedSnippet {
    pub content: Box<str>,
    pub content_type: HeaderValue,
    pub cache_mode: CacheMode,
    /// The content block hash for this version. Used to compute the strong ETag.
    /// Authoritative while cached: a Control Plane write invalidates the entry,
    /// so a present entry always carries the current hash.
    pub target_hash: Box<str>,
}

impl CachedSnippet {
    /// Approximate heap footprint, used as the cache weight. The constant
    /// covers the `Arc` allocations, the moka key, node overhead, and the
    /// fixed-width 64-byte signed content id stored in `target_hash` (always
    /// exactly 64 chars), so only the variable-length fields are measured.
    fn weight(&self) -> u32 {
        const OVERHEAD: usize = 224;
        (self.content.len() + self.content_type.as_bytes().len() + OVERHEAD).min(u32::MAX as usize)
            as u32
    }
}

impl From<DeliveryRecord> for CachedSnippet {
    fn from(record: DeliveryRecord) -> Self {
        Self {
            content: record.content.into_boxed_str(),
            content_type: content_type_header(&record.content_type),
            cache_mode: record.cache_mode,
            target_hash: record.target_hash.into_boxed_str(),
        }
    }
}

fn content_type_header(content_type: &str) -> HeaderValue {
    HeaderValue::from_str(content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("text/plain; charset=utf-8"))
}

/// The shared, byte-bounded delivery cache. Cheap to clone (`Arc` inside moka).
///
/// An entry leaves the cache only by:
/// 1. **Control Plane invalidation** — an explicit [`Self::invalidate`] call on
///    every write (the sole freshness guarantee).
/// 2. **Byte-budget pressure** — moka's weigher evicts the least-recently-used
///    entry when the total weight exceeds `byte_budget`.
///
/// There is no time-based expiry: a present entry is always current.
#[derive(Clone)]
pub struct DeliveryCache {
    inner: Cache<RouteId, Arc<CachedSnippet>>,
}

impl DeliveryCache {
    /// Build a cache bounded to `byte_budget` total weight. Entries are never
    /// time-expired; freshness is guaranteed by Control Plane invalidation.
    #[must_use]
    pub fn new(byte_budget: u64) -> Self {
        let inner = Cache::builder()
            .max_capacity(byte_budget)
            .weigher(|_key: &RouteId, value: &Arc<CachedSnippet>| value.weight())
            .build();
        Self { inner }
    }

    /// Read through the cache, loading on a miss. Concurrent misses for the
    /// same id are coalesced by moka into a single `load` invocation.
    ///
    /// Any `Err` from `load` is propagated to every waiter and is **not**
    /// cached. Callers therefore model "route not found" as an error variant
    /// (which maps to `404`), keeping negatives out of the cache for free while
    /// still distinguishing them from genuine load failures (`500`).
    pub async fn get_or_load<F, Fut, E>(
        &self,
        id: &RouteId,
        load: F,
    ) -> Result<Arc<CachedSnippet>, Arc<E>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<CachedSnippet, E>>,
        E: Send + Sync + 'static,
    {
        self.inner
            .try_get_with_by_ref(id, async { load().await.map(Arc::new) })
            .await
    }

    /// Evict a route from the cache. Called by the Control Plane on every write
    /// so the next Data Plane read observes the new content immediately.
    pub async fn invalidate(&self, id: &RouteId) {
        self.inner.invalidate(id).await;
    }

    /// Force any pending eviction/insertion bookkeeping to complete. Used by
    /// tests to make weight-based eviction observable deterministically.
    #[cfg(test)]
    async fn sync(&self) {
        self.inner.run_pending_tasks().await;
    }

    /// Current number of cached entries (after pending tasks settle).
    #[cfg(test)]
    async fn entry_count(&self) -> u64 {
        self.inner.run_pending_tasks().await;
        self.inner.entry_count()
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use super::*;
    use crate::crypto::IdSigner;

    fn snippet(content: &str, mode: CacheMode) -> CachedSnippet {
        CachedSnippet {
            content: Box::from(content),
            content_type: HeaderValue::from_static("text/plain; charset=utf-8"),
            cache_mode: mode,
            target_hash: "a".repeat(64).into_boxed_str(),
        }
    }

    /// Mint a fresh, valid signed route id for cache-keying in tests.
    fn test_id() -> RouteId {
        let signer = IdSigner::new("cache-test-secret-cache-test-secret");
        RouteId::from_signed(signer.random_id())
    }

    /// A loader error used to exercise the not-found / failure paths.
    #[derive(Debug, PartialEq, Eq)]
    enum TestError {
        NotFound,
    }

    async fn ok_loader(
        s: CachedSnippet,
    ) -> impl FnOnce() -> std::future::Ready<Result<CachedSnippet, Infallible>> {
        move || std::future::ready(Ok(s))
    }

    #[tokio::test]
    async fn miss_then_hit_loads_once() {
        let cache = DeliveryCache::new(1 << 20);
        let id = test_id();

        let first = cache
            .get_or_load(
                &id,
                ok_loader(snippet("payload", CacheMode::Immutable)).await,
            )
            .await
            .unwrap();
        assert_eq!(&*first.content, "payload");

        // Second call must hit; the loader would return different content if run.
        let second = cache
            .get_or_load(&id, || {
                std::future::ready(Result::<_, Infallible>::Ok(snippet(
                    "SHOULD NOT LOAD",
                    CacheMode::Immutable,
                )))
            })
            .await
            .unwrap();
        assert_eq!(&*second.content, "payload");
    }

    #[tokio::test]
    async fn invalidate_forces_reload() {
        let cache = DeliveryCache::new(1 << 20);
        let id = test_id();

        cache
            .get_or_load(&id, ok_loader(snippet("v1", CacheMode::Mutable)).await)
            .await
            .unwrap();
        cache.invalidate(&id).await;

        let reloaded = cache
            .get_or_load(&id, ok_loader(snippet("v2", CacheMode::Mutable)).await)
            .await
            .unwrap();
        assert_eq!(&*reloaded.content, "v2", "post-invalidation read is fresh");
    }

    #[tokio::test]
    async fn errors_are_not_cached() {
        let cache = DeliveryCache::new(1 << 20);
        let id = test_id();

        // First load fails (e.g. route not found) — must not be cached.
        let err = cache
            .get_or_load(&id, || {
                std::future::ready(Result::<CachedSnippet, _>::Err(TestError::NotFound))
            })
            .await
            .unwrap_err();
        assert_eq!(*err, TestError::NotFound);
        assert_eq!(cache.entry_count().await, 0, "negatives are not stored");

        // A subsequent successful load now populates the entry.
        let ok = cache
            .get_or_load(
                &id,
                ok_loader(snippet("now here", CacheMode::Mutable)).await,
            )
            .await
            .unwrap();
        assert_eq!(&*ok.content, "now here");
    }

    #[tokio::test]
    async fn byte_budget_evicts_large_entries() {
        // Budget fits roughly two ~1 KiB entries; inserting more must evict.
        let cache = DeliveryCache::new(2_500);
        let big = "x".repeat(1_000);

        for _ in 0..10 {
            let id = test_id();
            cache
                .get_or_load(&id, ok_loader(snippet(&big, CacheMode::Immutable)).await)
                .await
                .unwrap();
        }
        cache.sync().await;

        assert!(
            cache.entry_count().await < 10,
            "weight bound must evict; got {}",
            cache.entry_count().await
        );
    }

    #[test]
    fn invalid_content_type_falls_back_when_cached() {
        let snippet = CachedSnippet::from(DeliveryRecord {
            content: "payload".to_owned(),
            content_type: "not\na\nheader".to_owned(),
            cache_mode: CacheMode::Mutable,
            target_hash: "a".repeat(64),
        });

        assert_eq!(
            snippet.content_type.to_str().unwrap(),
            "text/plain; charset=utf-8"
        );
    }
}
