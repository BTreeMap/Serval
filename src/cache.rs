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
//! Three further properties keep the hot path cheap under read spikes:
//!
//! * **Pointer-sized reads.** The value is an [`Arc<CachedSnippet>`] whose
//!   content is itself an `Arc<str>`. A cache hit clones two atomics, never the
//!   20 KiB blob.
//! * **Stampede coalescing.** [`Cache::get_with`] collapses a thundering herd of
//!   concurrent misses for the same id into a single database load.
//! * **Mode-aware TTL.** Mutable entries carry a short TTL as a safety net
//!   behind explicit invalidation; immutable (content-addressed) entries never
//!   go stale and are not time-expired.
//!
//! [`weigher`]: moka::future::CacheBuilder::weigher

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use moka::Expiry;
use moka::future::Cache;

use crate::db::models::{CacheMode, DeliveryRecord, RouteId};

/// An immutable, shareable snapshot of everything needed to serve a route.
///
/// Cloning is `O(1)`: only the inner `Arc`s are bumped. Rendering happens per
/// request against the borrowed `content`, so the cache never stores rendered
/// output (which would vary by query string).
#[derive(Debug, Clone)]
pub struct CachedSnippet {
    pub content: Arc<str>,
    pub content_type: Arc<str>,
    pub cache_mode: CacheMode,
}

impl CachedSnippet {
    /// Approximate heap footprint, used as the cache weight. The constant
    /// covers the two `Arc` allocations, the key string, and node overhead;
    /// exactness is unnecessary since this only drives eviction pressure.
    fn weight(&self) -> u32 {
        const OVERHEAD: usize = 160;
        (self.content.len() + self.content_type.len() + OVERHEAD).min(u32::MAX as usize) as u32
    }
}

impl From<DeliveryRecord> for CachedSnippet {
    fn from(record: DeliveryRecord) -> Self {
        Self {
            content: Arc::from(record.content),
            content_type: Arc::from(record.content_type),
            cache_mode: record.cache_mode,
        }
    }
}

/// Per-entry expiry policy: immutable entries never expire; mutable entries get
/// a short TTL as a backstop behind explicit Control Plane invalidation.
struct ModeAwareExpiry {
    mutable_ttl: Duration,
}

impl Expiry<RouteId, Arc<CachedSnippet>> for ModeAwareExpiry {
    fn expire_after_create(
        &self,
        _key: &RouteId,
        value: &Arc<CachedSnippet>,
        _created_at: std::time::Instant,
    ) -> Option<Duration> {
        match value.cache_mode {
            CacheMode::Mutable => Some(self.mutable_ttl),
            CacheMode::Immutable => None,
        }
    }
}

/// The shared, byte-bounded delivery cache. Cheap to clone (`Arc` inside moka).
#[derive(Clone)]
pub struct DeliveryCache {
    inner: Cache<RouteId, Arc<CachedSnippet>>,
}

impl DeliveryCache {
    /// Build a cache bounded to `byte_budget` total weight, with `mutable_ttl`
    /// applied to mutable entries only.
    #[must_use]
    pub fn new(byte_budget: u64, mutable_ttl: Duration) -> Self {
        let inner = Cache::builder()
            .max_capacity(byte_budget)
            .weigher(|_key: &RouteId, value: &Arc<CachedSnippet>| value.weight())
            .expire_after(ModeAwareExpiry { mutable_ttl })
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

    fn snippet(content: &str, mode: CacheMode) -> CachedSnippet {
        CachedSnippet {
            content: Arc::from(content),
            content_type: Arc::from("text/plain; charset=utf-8"),
            cache_mode: mode,
        }
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
        let cache = DeliveryCache::new(1 << 20, Duration::from_secs(300));
        let id = RouteId::new_alias();

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
        let cache = DeliveryCache::new(1 << 20, Duration::from_secs(300));
        let id = RouteId::new_alias();

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
        let cache = DeliveryCache::new(1 << 20, Duration::from_secs(300));
        let id = RouteId::new_alias();

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
        let cache = DeliveryCache::new(2_500, Duration::from_secs(300));
        let big = "x".repeat(1_000);

        for _ in 0..10 {
            let id = RouteId::new_alias();
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
}
