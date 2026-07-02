//! Idempotent startup schema.
//!
//! Serval has no separate migration runner: the schema is (re)applied on every
//! boot using `CREATE ... IF NOT EXISTS` and guarded `ALTER TABLE`. This is the
//! migration mechanism mandated by the database contract — additive only, and
//! always safe to run against a populated database.
//!
//! The three tables encode the CAS model:
//! * `content_blocks` — immutable, content-addressed payloads (write-once).
//! * `routes` — the active, editable snippet pointers and their metadata.
//! * `pointer_history` — the infinite, append-only audit ledger.
//!
//! A fourth, additive `users` table tracks authenticated identities and the
//! locally administered admin role (OAuth providers cannot always express it).

use sqlx::PgPool;

/// Apply the full schema. Safe to call on every startup.
pub async fn apply(pool: &PgPool) -> Result<(), sqlx::Error> {
    // Immutable blob layer. Addressed by a signed content id:
    // Base64URL(BLAKE3(content) || keyed-MAC), 64 chars.
    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS content_blocks (
            hash_id VARCHAR(64) PRIMARY KEY,
            content TEXT NOT NULL
        )
        ",
    )
    .execute(pool)
    .await?;

    // Active routing layer. Each row is an editable snippet addressed by an
    // unguessable, signed id and pointing at its current content block.
    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS routes (
            id VARCHAR(64) PRIMARY KEY,
            target_hash VARCHAR(64) NOT NULL REFERENCES content_blocks (hash_id),
            content_type VARCHAR(255) NOT NULL DEFAULT 'text/plain; charset=utf-8',
            title VARCHAR(255),
            description TEXT,
            owner_id VARCHAR(255)
        )
        ",
    )
    .execute(pool)
    .await?;

    // Idempotent migration: add title and description to tables created before
    // this feature. Safe to run against tables that already have these columns.
    sqlx::query("ALTER TABLE routes ADD COLUMN IF NOT EXISTS title VARCHAR(255)")
        .execute(pool)
        .await?;
    sqlx::query("ALTER TABLE routes ADD COLUMN IF NOT EXISTS description TEXT")
        .execute(pool)
        .await?;

    // Append-only version ledger. Never pruned.
    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS pointer_history (
            id SERIAL PRIMARY KEY,
            route_id VARCHAR(64) NOT NULL REFERENCES routes (id),
            target_hash VARCHAR(64) NOT NULL REFERENCES content_blocks (hash_id),
            editor_id VARCHAR(255) NOT NULL,
            changed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        ",
    )
    .execute(pool)
    .await?;

    // Accelerates the audit-trail lookup for a given route in chronological
    // order without scanning the whole ledger.
    sqlx::query(
        r"
        CREATE INDEX IF NOT EXISTS pointer_history_route_changed_idx
            ON pointer_history (route_id, changed_at)
        ",
    )
    .execute(pool)
    .await?;

    // Accelerates the paginated (keyset) history scan for one route: rows are
    // read in `changed_at DESC, id DESC` order, matching the query's ORDER BY
    // exactly, so a page boundary is a single index range scan.
    sqlx::query(
        r"
        CREATE INDEX IF NOT EXISTS pointer_history_route_changed_id_idx
            ON pointer_history (route_id, changed_at DESC, id DESC)
        ",
    )
    .execute(pool)
    .await?;

    // Narrows the owner's snippet listing to their own routes before the sort;
    // the listing itself still orders by each route's latest ledger entry.
    sqlx::query(
        r"
        CREATE INDEX IF NOT EXISTS routes_owner_id_idx
            ON routes (owner_id, id)
        ",
    )
    .execute(pool)
    .await?;

    // Locally tracked authenticated users. Upserted on login; `is_admin` is
    // administered locally and is independent of any OAuth provider claim.
    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS users (
            id VARCHAR(255) PRIMARY KEY,
            is_admin BOOLEAN NOT NULL DEFAULT FALSE,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        ",
    )
    .execute(pool)
    .await?;

    Ok(())
}
