//! Shared application state for the two planes.
//!
//! Both planes read from the same PostgreSQL pool (via [`Repository`]) and the
//! same in-memory [`DeliveryCache`]. The cache handle is shared so that a
//! Control Plane write can evict an entry that a Data Plane read will observe —
//! this single shared handle is what makes cross-thread invalidation immediate
//! without any channel or coarse lock.

use std::sync::Arc;
use std::time::Duration;

use crate::auth::AuthService;
use crate::cache::DeliveryCache;
use crate::crypto::IdSigner;
use crate::db::Repository;

/// State for the Control Plane (management API + embedded UI).
#[derive(Clone)]
pub struct ControlState {
    pub repo: Repository,
    pub cache: DeliveryCache,
    pub auth: Arc<AuthService>,
    /// Mints signed route ids for newly created snippets.
    pub signer: IdSigner,
    /// Public base URL of the Data Plane, advertised to the dashboard so it can
    /// build delivery links even when the planes live on different domains.
    /// `None` lets the dashboard fall back to guessing from its own origin.
    pub data_plane_url: Option<Arc<str>>,
}

/// State for the Data Plane (public delivery).
#[derive(Clone)]
pub struct DeliveryState {
    pub repo: Repository,
    pub cache: DeliveryCache,
    /// Verifies the route-id MAC before any cache or database lookup.
    pub signer: IdSigner,
    /// Whether to serve stale mutable entries immediately while a background
    /// single-flight refresh brings the cache up to date.
    pub serve_stale: bool,
    /// Base freshness window for mutable cache entries. Used to compute
    /// `Cache-Control: stale-while-revalidate` and staleness checks.
    pub mutable_ttl: Duration,
}
