//! Database access: connection pool, schema, models, and the CAS repository.

pub mod models;
mod repo;
mod schema;

use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

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
