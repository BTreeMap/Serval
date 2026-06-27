//! Serval — high-performance snippet delivery and templating over a pure
//! content-addressed storage engine.
//!
//! The crate is split into a Control Plane (management API + embedded UI) and a
//! Data Plane (public delivery) sharing a PostgreSQL pool and an in-memory
//! delivery cache. This module tree exposes the building blocks; see the binary
//! entry point for how the two planes are wired together.

pub mod crypto;
pub mod db;
pub mod renderer;
