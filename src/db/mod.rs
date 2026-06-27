//! Database access: connection pool, schema, models, and the CAS repository.

pub mod models;
mod repo;
mod schema;

use std::str::FromStr;
use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

pub use repo::{CreateRoute, Repository};

/// Connect to PostgreSQL and apply the idempotent startup schema.
///
/// All persistence flows through the single returned pool; handlers must never
/// open ad-hoc connections of their own.
pub async fn connect(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect(database_url)
        .await?;

    schema::apply(&pool).await?;

    Ok(pool)
}

/// Connect a read-only pool for the public Data Plane.
///
/// Every connection is pinned to `default_transaction_read_only = on`, so
/// PostgreSQL itself rejects any `INSERT`/`UPDATE`/`DELETE`/DDL the moment a
/// data-plane code path attempts a write — a server-enforced guarantee that a
/// compromise of the public plane cannot mutate storage, independent of the
/// Rust types. The schema is intentionally *not* applied here; migrations are
/// the Control Plane's responsibility through [`connect`].
pub async fn connect_read_only(
    database_url: &str,
    max_connections: u32,
) -> Result<PgPool, sqlx::Error> {
    let options = PgConnectOptions::from_str(database_url)?
        .options([("default_transaction_read_only", "on")]);

    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await
}
