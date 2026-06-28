//! The content-addressed storage repository.
//!
//! Every write that touches more than one table runs inside a single
//! transaction so the blob, the route pointer, and the history ledger can never
//! drift out of sync. The repository upholds the system's hard invariants:
//!
//! * `content_blocks` is write-once (`ON CONFLICT DO NOTHING`); blocks are never
//!   updated or deleted.
//! * Creating a route appends version 1 to `pointer_history`; each update
//!   appends exactly one further row. The ledger is never pruned.
//! * Every snippet is an editable route; a content hash is itself a valid id,
//!   so a specific stored version is addressable directly by its hash.

use sqlx::PgPool;

use super::models::{
    CacheMode, ContentHash, DeliveryRecord, HistoryEntry, RouteId, RouteMeta, RouteSummary, User,
};

/// Parameters for creating a new route over a freshly stored content block.
pub struct CreateRoute<'a> {
    pub id: RouteId,
    pub hash: ContentHash,
    pub content: &'a str,
    pub content_type: &'a str,
    pub owner_id: Option<&'a str>,
    pub editor_id: &'a str,
}

/// CAS repository over a shared PostgreSQL pool.
#[derive(Clone)]
pub struct Repository {
    pool: PgPool,
}

impl Repository {
    /// Wrap a pool in the repository interface.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Persist a content block and a new route pointing at it, recording the
    /// initial version in the history ledger — all atomically.
    ///
    /// Returns `Ok(false)` if the route id already exists (the caller chose a
    /// colliding alias, or re-created an identical permalink); in that case no
    /// new history row is written.
    pub async fn create_route(&self, params: CreateRoute<'_>) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        // Write-once blob insert; identical content collapses to one row.
        sqlx::query(
            "INSERT INTO content_blocks (hash_id, content) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(params.hash.as_str())
        .bind(params.content)
        .execute(&mut *tx)
        .await?;

        let inserted = sqlx::query(
            r"
            INSERT INTO routes (id, target_hash, content_type, owner_id)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (id) DO NOTHING
            ",
        )
        .bind(params.id.as_str())
        .bind(params.hash.as_str())
        .bind(params.content_type)
        .bind(params.owner_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();

        if inserted == 0 {
            // Route already existed; leave the ledger untouched.
            tx.rollback().await?;
            return Ok(false);
        }

        self.append_history(&mut tx, &params.id, &params.hash, params.editor_id)
            .await?;

        tx.commit().await?;
        Ok(true)
    }

    /// Repoint an existing mutable route at new content, appending one row to
    /// the history ledger. Returns `Ok(false)` if the route does not exist.
    ///
    /// `hash` is the caller-computed content id (`signer.content_id(content)`);
    /// the repository stays key-free and never derives ids itself.
    pub async fn update_route(
        &self,
        id: &RouteId,
        hash: &ContentHash,
        content: &str,
        editor_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO content_blocks (hash_id, content) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(hash.as_str())
        .bind(content)
        .execute(&mut *tx)
        .await?;

        let updated = sqlx::query("UPDATE routes SET target_hash = $2 WHERE id = $1")
            .bind(id.as_str())
            .bind(hash.as_str())
            .execute(&mut *tx)
            .await?
            .rows_affected();

        if updated == 0 {
            tx.rollback().await?;
            return Ok(false);
        }

        self.append_history(&mut tx, id, hash, editor_id).await?;

        tx.commit().await?;
        Ok(true)
    }

    /// The Data Plane read path: resolve a live route to its current content
    /// and presentation metadata via the index join. A live route is always
    /// cached as [`CacheMode::Mutable`] — it may be repointed by its owner.
    pub async fn fetch_delivery(
        &self,
        id: &RouteId,
    ) -> Result<Option<DeliveryRecord>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String)>(
            r"
            SELECT c.content, r.content_type
            FROM routes r
            INNER JOIN content_blocks c ON c.hash_id = r.target_hash
            WHERE r.id = $1
            ",
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(content, content_type)| DeliveryRecord {
            content,
            content_type,
            cache_mode: CacheMode::Mutable,
        }))
    }

    /// The content-addressed delivery fallback: resolve an id directly to the
    /// immutable stored block it names, independent of any route. This is the
    /// internal "version permalink" path — a content hash addresses exactly one
    /// version, so it is served as [`CacheMode::Immutable`]. The block carries
    /// no presentation metadata, so the inert default content type is used
    /// (a cosmetic filename extension can still drive the response MIME).
    pub async fn fetch_content_block(
        &self,
        hash: &ContentHash,
    ) -> Result<Option<DeliveryRecord>, sqlx::Error> {
        let row =
            sqlx::query_as::<_, (String,)>("SELECT content FROM content_blocks WHERE hash_id = $1")
                .bind(hash.as_str())
                .fetch_optional(&self.pool)
                .await?;

        Ok(row.map(|(content,)| DeliveryRecord {
            content,
            content_type: "text/plain; charset=utf-8".to_owned(),
            cache_mode: CacheMode::Immutable,
        }))
    }

    /// Count the history rows recorded for a route. Used by the audit view and
    /// by the infinite-ledger acceptance test.
    pub async fn history_count(&self, id: &RouteId) -> Result<i64, sqlx::Error> {
        let (count,) =
            sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM pointer_history WHERE route_id = $1")
                .bind(id.as_str())
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }

    /// Record a login: insert the user on first sight, otherwise refresh their
    /// `last_seen_at`. The admin flag is never changed here — it is local state.
    pub async fn upsert_user(&self, id: &str) -> Result<User, sqlx::Error> {
        let row = sqlx::query_as::<
            _,
            (
                String,
                bool,
                chrono::DateTime<chrono::Utc>,
                chrono::DateTime<chrono::Utc>,
            ),
        >(
            r"
            INSERT INTO users (id) VALUES ($1)
            ON CONFLICT (id) DO UPDATE SET last_seen_at = NOW()
            RETURNING id, is_admin, created_at, last_seen_at
            ",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        Ok(User {
            id: row.0,
            is_admin: row.1,
            created_at: row.2,
            last_seen_at: row.3,
        })
    }

    /// Whether the given user currently holds the admin role. Unknown users are
    /// treated as non-admins.
    pub async fn is_admin(&self, id: &str) -> Result<bool, sqlx::Error> {
        let row = sqlx::query_as::<_, (bool,)>("SELECT is_admin FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some_and(|(is_admin,)| is_admin))
    }

    /// Set or clear a user's admin role locally, creating the user row if it
    /// does not yet exist. This is the out-of-band escape hatch for providers
    /// that cannot express an admin claim.
    pub async fn set_admin(&self, id: &str, is_admin: bool) -> Result<(), sqlx::Error> {
        sqlx::query(
            r"
            INSERT INTO users (id, is_admin) VALUES ($1, $2)
            ON CONFLICT (id) DO UPDATE SET is_admin = EXCLUDED.is_admin
            ",
        )
        .bind(id)
        .bind(is_admin)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// List all users holding the admin role, most recently seen first.
    pub async fn list_admins(&self) -> Result<Vec<User>, sqlx::Error> {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                bool,
                chrono::DateTime<chrono::Utc>,
                chrono::DateTime<chrono::Utc>,
            ),
        >(
            r"
            SELECT id, is_admin, created_at, last_seen_at
            FROM users
            WHERE is_admin = TRUE
            ORDER BY last_seen_at DESC
            ",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(id, is_admin, created_at, last_seen_at)| User {
                id,
                is_admin,
                created_at,
                last_seen_at,
            })
            .collect())
    }

    /// Fetch routing-layer metadata for a route (no content). Returns `None`
    /// when the route does not exist.
    pub async fn fetch_route_meta(&self, id: &RouteId) -> Result<Option<RouteMeta>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT target_hash, content_type, owner_id FROM routes WHERE id = $1",
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(target_hash, content_type, owner_id)| RouteMeta {
            target_hash,
            content_type,
            owner_id,
        }))
    }

    /// List a user's routes, most recently changed first. The "last changed"
    /// timestamp is read from the head of each route's history ledger, so the
    /// listing reflects updates without the `routes` table carrying a mutable
    /// timestamp column.
    pub async fn list_routes_for_owner(
        &self,
        owner_id: &str,
    ) -> Result<Vec<RouteSummary>, sqlx::Error> {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                String,
                Option<String>,
                chrono::DateTime<chrono::Utc>,
            ),
        >(
            r"
            SELECT r.id,
                   r.content_type,
                   r.owner_id,
                   COALESCE(MAX(h.changed_at), NOW()) AS updated_at
            FROM routes r
            LEFT JOIN pointer_history h ON h.route_id = r.id
            WHERE r.owner_id = $1
            GROUP BY r.id, r.content_type, r.owner_id
            ORDER BY updated_at DESC
            ",
        )
        .bind(owner_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(id, content_type, owner_id, updated_at)| RouteSummary {
                id,
                content_type,
                owner_id,
                updated_at,
            })
            .collect())
    }

    /// List the full version history of a route, newest first.
    pub async fn list_history(&self, id: &RouteId) -> Result<Vec<HistoryEntry>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String, chrono::DateTime<chrono::Utc>)>(
            r"
            SELECT target_hash, editor_id, changed_at
            FROM pointer_history
            WHERE route_id = $1
            ORDER BY changed_at DESC, id DESC
            ",
        )
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(target_hash, editor_id, changed_at)| HistoryEntry {
                target_hash,
                editor_id,
                changed_at,
            })
            .collect())
    }

    /// Fetch the content of one historical version of a route, identified by
    /// the version's content hash. Returns `None` unless that hash is a genuine
    /// version of *this* route, so a caller can never read arbitrary content
    /// through another route's id.
    pub async fn fetch_version_content(
        &self,
        id: &RouteId,
        hash: &ContentHash,
    ) -> Result<Option<String>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String,)>(
            r"
            SELECT c.content
            FROM content_blocks c
            WHERE c.hash_id = $2
              AND EXISTS (
                  SELECT 1 FROM pointer_history h
                  WHERE h.route_id = $1 AND h.target_hash = $2
              )
            ",
        )
        .bind(id.as_str())
        .bind(hash.as_str())
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(content,)| content))
    }

    /// Repoint a route at one of its own historical versions, appending the
    /// restore to the ledger as a new version. Returns `Ok(false)` when `hash`
    /// is not a known version of this route (or the route does not exist); the
    /// content block is already stored, so none is inserted.
    pub async fn restore_version(
        &self,
        id: &RouteId,
        hash: &ContentHash,
        editor_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        // The target must be a genuine prior version of this exact route.
        let (is_version,) = sqlx::query_as::<_, (bool,)>(
            r"
            SELECT EXISTS (
                SELECT 1 FROM pointer_history
                WHERE route_id = $1 AND target_hash = $2
            )
            ",
        )
        .bind(id.as_str())
        .bind(hash.as_str())
        .fetch_one(&mut *tx)
        .await?;

        if !is_version {
            tx.rollback().await?;
            return Ok(false);
        }

        let updated = sqlx::query("UPDATE routes SET target_hash = $2 WHERE id = $1")
            .bind(id.as_str())
            .bind(hash.as_str())
            .execute(&mut *tx)
            .await?
            .rows_affected();

        if updated == 0 {
            tx.rollback().await?;
            return Ok(false);
        }

        self.append_history(&mut tx, id, hash, editor_id).await?;

        tx.commit().await?;
        Ok(true)
    }

    /// Append one row to the append-only ledger within the caller's transaction.
    async fn append_history(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        route_id: &RouteId,
        target_hash: &ContentHash,
        editor_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r"
            INSERT INTO pointer_history (route_id, target_hash, editor_id)
            VALUES ($1, $2, $3)
            ",
        )
        .bind(route_id.as_str())
        .bind(target_hash.as_str())
        .bind(editor_id)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
}
