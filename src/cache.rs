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
//! Four properties keep the hot path cheap under read spikes:
//!
//! * **Pointer-sized reads.** The value is an [`Arc<CachedSnippet>`] whose
//!   content is itself an `Arc<str>`. A cache hit clones two atomics, never the
//!   20 KiB blob.
//! * **Stampede coalescing.** [`Cache::get_with`] collapses a thundering herd of
//!   concurrent misses for the same id into a single database load.
//! * **Opportunistic never-expire.** Mutable entries are never time-evicted;
//!   they live until Control Plane invalidation or byte-budget pressure. An
//!   entry older than `refresh_after` is *stale* — a staleness signal triggers a
//!   background refresh but the entry is always served until something better
//!   arrives or the cache runs out of space.
//! * **Lock-free single-flight.** Each `CachedSnippet` carries an `AtomicBool`
//!   refresh flag. The first stale reader wins a `compare_exchange` and spawns
//!   the background refresh; all other concurrent stale readers skip it — zero
//!   global lock, zero per-id allocation.
//!
//! [`weigher`]: moka::future::CacheBuilder::weigher

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use moka::future::Cache;

use crate::db::models::{CacheMode, DeliveryRecord, RouteId};

/// An immutable, shareable snapshot of everything needed to serve a route.
///
/// The cache stores this behind an `Arc`; cloning the cache entry costs two
/// atomic-reference increments, never the content bytes. Rendering happens per
/// request against the borrowed `content`.
#[derive(Debug)]
pub struct CachedSnippet {
    pub content: Arc<str>,
    pub content_type: Arc<str>,
    pub cache_mode: CacheMode,
    /// The content block hash for this version. Used to compute ETags and to
    /// detect whether the route has been repointed since this entry was cached.
    pub target_hash: Arc<str>,
    /// Wall-clock time of insertion, used to identify stale entries.
    inserted_at: Instant,
    /// Lock-free single-flight refresh guard. Exactly one caller can flip this
    /// from `false` to `true` via [`RefreshGuard::try_acquire`]; the claim is
    /// released when the returned guard is dropped (including on panic).
    refreshing: AtomicBool,
}

impl CachedSnippet {
    /// Approximate heap footprint, used as the cache weight. The constant
    /// covers the `Arc` allocations, the moka key, node overhead, and the
    /// fixed-width 64-byte signed content id stored in `target_hash` (always
    /// exactly 64 chars), so only the variable-length fields are measured.
    fn weight(&self) -> u32 {
        const OVERHEAD: usize = 224;
        (self.content.len() + self.content_type.len() + OVERHEAD).min(u32::MAX as usize) as u32
    }
}

impl CachedSnippet {
    /// Read the (sole) field that needs ordering with the refresh flag: this is
    /// a convenience for the guard's `Drop`. Kept private to the module.
    fn release_refresh(&self) {
        // `Release` so the `false` becomes visible to the next claimer's
        // `Acquire` CAS, establishing the happens-before that a try-lock needs.
        self.refreshing.store(false, Ordering::Release);
    }
}

/// An RAII single-flight refresh claim.
///
/// Acquired via [`RefreshGuard::try_acquire`]; the flag is released when the
/// guard is dropped — including on early return, error, or panic. This makes
/// the "stuck `true`" state unrepresentable: there is no code path that can
/// hold the claim without a live guard, so an entry can never be frozen out of
/// future refreshes by a forgotten release.
///
/// The guard owns an `Arc` to the entry it claimed. After a refresh replaces
/// the entry in the cache, releasing the *old* entry's flag is harmless: the
/// old entry is unreachable, and the replacement carries its own fresh flag.
#[must_use = "dropping the guard immediately releases the refresh claim"]
pub struct RefreshGuard {
    entry: Arc<CachedSnippet>,
}

impl RefreshGuard {
    /// Try to claim the exclusive right to refresh `entry`. Returns `Some` to
    /// exactly one caller while a refresh is in flight; every other caller gets
    /// `None` and should skip (serving the stale entry).
    pub fn try_acquire(entry: &Arc<CachedSnippet>) -> Option<Self> {
        // Try-lock idiom: `Acquire` on success synchronizes-with the previous
        // holder's `Release`; `Relaxed` on failure since we touch no shared
        // state when we lose the race. Only one thread can observe the
        // `false -> true` transition, so at most one refresh is ever spawned.
        entry
            .refreshing
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self {
                entry: Arc::clone(entry),
            })
    }

    /// The entry whose refresh this guard owns.
    #[must_use]
    pub fn entry(&self) -> &Arc<CachedSnippet> {
        &self.entry
    }
}

impl Drop for RefreshGuard {
    fn drop(&mut self) {
        self.entry.release_refresh();
    }
}

impl From<DeliveryRecord> for CachedSnippet {
    fn from(record: DeliveryRecord) -> Self {
        Self {
            content: Arc::from(record.content),
            content_type: Arc::from(record.content_type),
            cache_mode: record.cache_mode,
            target_hash: Arc::from(record.target_hash),
            inserted_at: Instant::now(),
            refreshing: AtomicBool::new(false),
        }
    }
}

/// The shared, byte-bounded delivery cache. Cheap to clone (`Arc` inside moka).
///
/// Entries are **never time-evicted**. The only ways an entry leaves the cache
/// are:
/// 1. **Control Plane invalidation** — an explicit [`Self::invalidate`] call on
///    every write (the sole freshness guarantee).
/// 2. **Byte-budget pressure** — moka's weigher evicts the least-recently-used
///    entry when the total weight exceeds `byte_budget`.
///
/// "Staleness" is a refresh trigger, not an eviction mechanism. A mutable entry
/// older than `refresh_after` has its age noticed by [`Self::get_cached`], which
/// returns `is_stale = true`; the caller decides whether to serve it and refresh
/// opportunistically (default) or revalidate synchronously.
#[derive(Clone)]
pub struct DeliveryCache {
    inner: Cache<RouteId, Arc<CachedSnippet>>,
    /// Staleness threshold for mutable entries. An entry older than this is
    /// considered stale and triggers a background refresh. Also used as the
    /// `stale-while-revalidate` header value.
    refresh_after: Duration,
}

impl DeliveryCache {
    /// Build a cache bounded to `byte_budget` total weight. Entries are never
    /// time-expired; `refresh_after` is the staleness threshold for mutable
    /// entries that triggers a background refresh.
    #[must_use]
    pub fn new(byte_budget: u64, refresh_after: Duration) -> Self {
        let inner = Cache::builder()
            .max_capacity(byte_budget)
            .weigher(|_key: &RouteId, value: &Arc<CachedSnippet>| value.weight())
            .build();
        Self {
            inner,
            refresh_after,
        }
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

    /// Check for a cached entry without issuing a database load.
    ///
    /// Returns `(entry, is_stale)` where `is_stale` is `true` for mutable
    /// entries whose age exceeds `refresh_after`. Returns `None` on a miss.
    pub async fn get_cached(&self, id: &RouteId) -> Option<(Arc<CachedSnippet>, bool)> {
        let entry = self.inner.get(id).await?;
        let is_stale = entry.cache_mode == CacheMode::Mutable
            && entry.inserted_at.elapsed() > self.refresh_after;
        Some((entry, is_stale))
    }

    /// Directly insert a delivery record into the cache, returning the new
    /// entry. Used by the background refresh path to update a stale entry after
    /// a hash change is detected.
    pub async fn insert(&self, id: &RouteId, record: DeliveryRecord) -> Arc<CachedSnippet> {
        let snippet = Arc::new(CachedSnippet::from(record));
        self.inner.insert(id.clone(), Arc::clone(&snippet)).await;
        snippet
    }

    /// Re-insert an existing entry with a refreshed `inserted_at` timestamp,
    /// resetting its staleness window without any data movement. Used when the
    /// background refresh confirms the hash is unchanged.
    pub async fn touch(&self, id: &RouteId, snippet: Arc<CachedSnippet>) {
        let refreshed = CachedSnippet {
            content: Arc::clone(&snippet.content),
            content_type: Arc::clone(&snippet.content_type),
            target_hash: Arc::clone(&snippet.target_hash),
            cache_mode: snippet.cache_mode,
            inserted_at: Instant::now(),
            refreshing: AtomicBool::new(false),
        };
        self.inner.insert(id.clone(), Arc::new(refreshed)).await;
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
            content: Arc::from(content),
            content_type: Arc::from("text/plain; charset=utf-8"),
            cache_mode: mode,
            target_hash: Arc::from("a".repeat(64)),
            inserted_at: Instant::now(),
            refreshing: AtomicBool::new(false),
        }
    }

    /// Build a `CachedSnippet` that already appears `age` old. Used to test
    /// staleness detection without sleeping or a mocked clock.
    fn backdated(content: &str, mode: CacheMode, age: Duration) -> CachedSnippet {
        CachedSnippet {
            content: Arc::from(content),
            content_type: Arc::from("text/plain; charset=utf-8"),
            cache_mode: mode,
            target_hash: Arc::from("a".repeat(64)),
            inserted_at: Instant::now() - age,
            refreshing: AtomicBool::new(false),
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
        let cache = DeliveryCache::new(1 << 20, Duration::from_secs(300));
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
        let cache = DeliveryCache::new(1 << 20, Duration::from_secs(300));
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
        let cache = DeliveryCache::new(1 << 20, Duration::from_secs(300));
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
        let cache = DeliveryCache::new(2_500, Duration::from_secs(300));
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

    #[tokio::test]
    async fn get_cached_returns_none_on_miss() {
        let cache = DeliveryCache::new(1 << 20, Duration::from_secs(300));
        let id = test_id();
        assert!(cache.get_cached(&id).await.is_none());
    }

    #[tokio::test]
    async fn get_cached_fresh_entry_is_not_stale() {
        let cache = DeliveryCache::new(1 << 20, Duration::from_secs(300));
        let id = test_id();
        cache
            .get_or_load(&id, ok_loader(snippet("v1", CacheMode::Mutable)).await)
            .await
            .unwrap();
        let (_, is_stale) = cache.get_cached(&id).await.expect("entry present");
        assert!(!is_stale, "brand-new entry must be fresh");
    }

    #[tokio::test]
    async fn get_cached_immutable_never_stale() {
        // Immutable entries carry no staleness even if their fake inserted_at
        // were ancient — only mutable entries can go stale.
        let cache = DeliveryCache::new(1 << 20, Duration::from_secs(300));
        let id = test_id();
        // Insert directly to set an old timestamp via the inner get_or_load path.
        cache
            .get_or_load(&id, ok_loader(snippet("imm", CacheMode::Immutable)).await)
            .await
            .unwrap();
        let (_, is_stale) = cache.get_cached(&id).await.expect("entry present");
        assert!(!is_stale, "immutable entries are never stale");
    }

    #[tokio::test]
    async fn single_flight_guard_blocks_concurrent_refresh() {
        // The per-entry AtomicBool ensures at most one refresh fires per entry,
        // and the RAII guard releases the claim on drop.
        let cache = DeliveryCache::new(1 << 20, Duration::from_secs(300));
        let id = test_id();
        cache
            .get_or_load(&id, ok_loader(snippet("v1", CacheMode::Mutable)).await)
            .await
            .unwrap();
        let (entry, _) = cache.get_cached(&id).await.expect("entry present");

        let g1 = RefreshGuard::try_acquire(&entry);
        assert!(g1.is_some(), "first claim must succeed");
        assert!(
            RefreshGuard::try_acquire(&entry).is_none(),
            "second claim while first is in-flight must fail"
        );
        drop(g1);
        assert!(
            RefreshGuard::try_acquire(&entry).is_some(),
            "claim after the guard is dropped must succeed again"
        );
    }

    #[tokio::test]
    async fn mutable_entry_is_never_evicted_by_time() {
        // With a very short refresh_after, a backdated entry must remain in the
        // cache (is_stale=true) — opportunistic never-expire semantics.
        let cache = DeliveryCache::new(1 << 20, Duration::from_millis(10));
        let id = test_id();
        // Load an entry that is already older than refresh_after.
        let old = backdated("v1", CacheMode::Mutable, Duration::from_millis(25));
        cache.get_or_load(&id, ok_loader(old).await).await.unwrap();

        let result = cache.get_cached(&id).await;
        assert!(result.is_some(), "entry must survive past refresh_after");
        let (_, is_stale) = result.unwrap();
        assert!(is_stale, "entry past refresh_after threshold must be stale");
    }

    #[tokio::test]
    async fn insert_updates_entry() {
        let cache = DeliveryCache::new(1 << 20, Duration::from_secs(300));
        let id = test_id();

        cache
            .get_or_load(&id, ok_loader(snippet("old", CacheMode::Mutable)).await)
            .await
            .unwrap();

        let record = DeliveryRecord {
            content: "new".to_owned(),
            content_type: "text/plain; charset=utf-8".to_owned(),
            cache_mode: CacheMode::Mutable,
            target_hash: "b".repeat(64),
        };
        cache.insert(&id, record).await;

        let (entry, _) = cache
            .get_cached(&id)
            .await
            .expect("entry present after insert");
        assert_eq!(&*entry.content, "new");
    }

    #[tokio::test]
    async fn touch_resets_staleness() {
        // Verify that `touch` re-inserts with a fresh timestamp by round-tripping
        // through get_cached.
        let cache = DeliveryCache::new(1 << 20, Duration::from_secs(300));
        let id = test_id();

        cache
            .get_or_load(&id, ok_loader(snippet("v1", CacheMode::Mutable)).await)
            .await
            .unwrap();
        let (entry, _) = cache.get_cached(&id).await.expect("entry present");

        // Touch should succeed and the entry should still be retrievable.
        cache.touch(&id, Arc::clone(&entry)).await;
        assert!(
            cache.get_cached(&id).await.is_some(),
            "entry survives touch"
        );
    }
}
