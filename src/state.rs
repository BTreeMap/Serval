//! Shared application state for the two planes.
//!
//! Both planes read from the same PostgreSQL pool (via [`Repository`]) and the
//! same in-memory [`DeliveryCache`]. The cache handle is shared so that a
//! Control Plane write can evict an entry that a Data Plane read will observe —
//! this single shared handle is what makes cross-thread invalidation immediate
//! without any channel or coarse lock.

use std::sync::Arc;

use crate::auth::AuthService;
use crate::cache::DeliveryCache;
use crate::db::Repository;

/// State for the Control Plane (management API + embedded UI).
#[derive(Clone)]
pub struct ControlState {
    pub repo: Repository,
    pub cache: DeliveryCache,
    pub auth: Arc<AuthService>,
}

/// State for the Data Plane (public delivery).
#[derive(Clone)]
pub struct DeliveryState {
    pub repo: Repository,
    pub cache: DeliveryCache,
}
