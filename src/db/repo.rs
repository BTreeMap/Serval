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
    CacheMode, ContentHash, DeliveryRecord, HistoryEntry, RouteAnnotations, RouteDetailPage,
    RouteId, RouteMeta, RouteSummary, User,
};

/// Parameters for creating a new route over a freshly stored content block.
pub struct CreateRoute<'a> {
    pub id: RouteId,
    pub hash: ContentHash,
    pub content: &'a str,
    pub content_type: &'a str,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub owner_id: Option<&'a str>,
    pub editor_id: &'a str,
}

/// Partial route metadata update. `None` means "leave as-is"; `Some(None)`
/// means "clear this nullable annotation".
pub struct RouteMetadataPatch<'a> {
    pub content_type: Option<&'a str>,
    pub title: Option<Option<&'a str>>,
    pub description: Option<Option<&'a str>>,
}

/// CAS repository over a shared PostgreSQL pool.
#[derive(Clone)]
pub struct Repository {
    pool: PgPool,
}

/// Inert default content type for blocks served without route metadata.
const DEFAULT_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

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

        let history_id = sqlx::query_scalar::<_, i64>(
            r"
            WITH inserted_route AS (
                INSERT INTO routes (id, target_hash, content_type, title, description, owner_id)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (id) DO NOTHING
                RETURNING id
            )
            INSERT INTO pointer_history (route_id, target_hash, editor_id)
            SELECT id, $2, $7
            FROM inserted_route
            RETURNING id
            ",
        )
        .bind(params.id.as_str())
        .bind(params.hash.as_str())
        .bind(params.content_type)
        .bind(params.title)
        .bind(params.description)
        .bind(params.owner_id)
        .bind(params.editor_id)
        .fetch_optional(&mut *tx)
        .await?;

        if history_id.is_none() {
            // Route already existed; leave the ledger untouched.
            tx.rollback().await?;
            return Ok(false);
        }

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

        let history_id = sqlx::query_scalar::<_, i64>(
            r"
            WITH updated_route AS (
                UPDATE routes
                SET target_hash = $2
                WHERE id = $1
                RETURNING id
            )
            INSERT INTO pointer_history (route_id, target_hash, editor_id)
            SELECT id, $2, $3
            FROM updated_route
            RETURNING id
            ",
        )
        .bind(id.as_str())
        .bind(hash.as_str())
        .bind(editor_id)
        .fetch_optional(&mut *tx)
        .await?;

        if history_id.is_none() {
            tx.rollback().await?;
            return Ok(false);
        }

        tx.commit().await?;
        Ok(true)
    }

    /// Update only route presentation metadata without touching the content
    /// pointer or history ledger. Each field has an explicit "set this field"
    /// boolean so nullable annotations can distinguish "clear to NULL" from
    /// "leave unchanged" in one stable query shape.
    pub async fn update_route_metadata(
        &self,
        id: &RouteId,
        patch: RouteMetadataPatch<'_>,
    ) -> Result<Option<RouteAnnotations>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
            r"
            UPDATE routes
            SET content_type = CASE WHEN $2 THEN $3::varchar ELSE content_type END,
                title = CASE WHEN $4 THEN $5::varchar ELSE title END,
                description = CASE WHEN $6 THEN $7::text ELSE description END
            WHERE id = $1
            RETURNING content_type, title, description
            ",
        )
        .bind(id.as_str())
        .bind(patch.content_type.is_some())
        .bind(patch.content_type)
        .bind(patch.title.is_some())
        .bind(patch.title.flatten())
        .bind(patch.description.is_some())
        .bind(patch.description.flatten())
        .fetch_optional(&self.pool)
        .await?;

        Ok(
            row.map(|(content_type, title, description)| RouteAnnotations {
                content_type,
                title,
                description,
            }),
        )
    }

    /// The Data Plane read path, resolved in a single round trip.
    ///
    /// A verified id resolves one of two disjoint ways, written as two
    /// primary-key probes under one `UNION ALL`:
    ///
    /// * **Live route** — the id owns a `routes` row, so its *current* content
    ///   and presentation metadata are served and cached as
    ///   [`CacheMode::Mutable`]: the owner may repoint it at any time.
    /// * **Content hash** — the id is itself a stored block's `hash_id`, naming
    ///   exactly one immutable version, served directly as
    ///   [`CacheMode::Immutable`]. A block carries no presentation metadata, so
    ///   the inert [`DEFAULT_CONTENT_TYPE`] is used (a cosmetic filename
    ///   extension can still drive the response MIME downstream).
    ///
    /// The 256-bit id prefix (CSPRNG route id vs. `BLAKE3` content hash) makes
    /// the two branches collision-free, so the query yields at most one row and
    /// a live route always wins. Both branches are unique-index scans on `$1`,
    /// so the plan is statistics-independent and stable at any data volume.
    pub async fn fetch_for_delivery(
        &self,
        id: &RouteId,
    ) -> Result<Option<DeliveryRecord>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, Option<String>, String, bool)>(
            r"
                SELECT c.content, r.content_type, r.target_hash, TRUE AS via_route
                FROM routes r
                JOIN content_blocks c ON c.hash_id = r.target_hash
                WHERE r.id = $1
            UNION ALL
                SELECT c.content, NULL::varchar, c.hash_id, FALSE AS via_route
                FROM content_blocks c
                WHERE c.hash_id = $1
            ",
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(content, content_type, target_hash, via_route)| {
            let (content_type, cache_mode) = if via_route {
                (
                    content_type.unwrap_or_else(|| DEFAULT_CONTENT_TYPE.to_owned()),
                    CacheMode::Mutable,
                )
            } else {
                (DEFAULT_CONTENT_TYPE.to_owned(), CacheMode::Immutable)
            };
            DeliveryRecord {
                content,
                content_type,
                cache_mode,
                target_hash,
            }
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
            SELECT id, TRUE AS is_admin, created_at, last_seen_at
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
        let row = sqlx::query_as::<_, (String, String, Option<String>, Option<String>, Option<String>)>(
            "SELECT target_hash, content_type, title, description, owner_id FROM routes WHERE id = $1",
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(target_hash, content_type, title, description, owner_id)| RouteMeta {
                target_hash,
                annotations: RouteAnnotations {
                    content_type,
                    title,
                    description,
                },
                owner_id,
            },
        ))
    }

    /// Fetch route metadata, the exact ledger count, and the newest bounded
    /// history page in one round trip for the snippet detail endpoint.
    pub async fn fetch_route_detail_page(
        &self,
        id: &RouteId,
        fetch_limit: i64,
    ) -> Result<Option<RouteDetailPage>, sqlx::Error> {
        type Row = (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<chrono::DateTime<chrono::Utc>>,
        );

        let rows = sqlx::query_as::<_, Row>(
            r"
            WITH route_meta AS (
                SELECT target_hash, content_type, title, description, owner_id
                FROM routes
                WHERE id = $1
            )
            SELECT rm.target_hash,
                   rm.content_type,
                   rm.title,
                   rm.description,
                   rm.owner_id,
                   totals.history_count,
                   history.id,
                   history.target_hash,
                   history.editor_id,
                   history.changed_at
            FROM route_meta rm
            CROSS JOIN LATERAL (
                SELECT COUNT(*) AS history_count
                FROM pointer_history
                WHERE route_id = $1
            ) totals
            LEFT JOIN LATERAL (
                SELECT id, target_hash, editor_id, changed_at
                FROM pointer_history
                WHERE route_id = $1
                ORDER BY changed_at DESC, id DESC
                LIMIT $2
            ) history ON TRUE
            ORDER BY history.changed_at DESC NULLS LAST, history.id DESC NULLS LAST
            ",
        )
        .bind(id.as_str())
        .bind(fetch_limit)
        .fetch_all(&self.pool)
        .await?;

        let Some((
            target_hash,
            content_type,
            title,
            description,
            owner_id,
            history_count,
            _,
            _,
            _,
            _,
        )) = rows.first()
        else {
            return Ok(None);
        };

        let meta = RouteMeta {
            target_hash: target_hash.clone(),
            annotations: RouteAnnotations {
                content_type: content_type.clone(),
                title: title.clone(),
                description: description.clone(),
            },
            owner_id: owner_id.clone(),
        };
        let history_count = *history_count;

        let history = rows
            .into_iter()
            .filter_map(
                |(_, _, _, _, _, _, id, target_hash, editor_id, changed_at)| {
                    Some(HistoryEntry {
                        id: id?,
                        target_hash: target_hash?,
                        editor_id: editor_id?,
                        changed_at: changed_at?,
                    })
                },
            )
            .collect();

        Ok(Some(RouteDetailPage {
            meta,
            history_count,
            history,
        }))
    }

    /// List a page of a user's routes, most recently changed first. The
    /// "last changed" timestamp is read from the head of each route's history
    /// ledger via a per-route `LATERAL` lookup, so the listing reflects
    /// updates without the `routes` table carrying a mutable timestamp column.
    ///
    /// `after` is the `(updated_at, id)` keyset tuple of the last row the
    /// caller already has — `None` fetches the first page. `fetch_limit`
    /// should be the caller's page size *plus one*, so the caller can detect
    /// whether another page follows without a second query. Both branches
    /// share the same `ORDER BY`, so the two queries are guaranteed to agree
    /// on row order at the page boundary.
    pub async fn list_routes_for_owner_page(
        &self,
        owner_id: &str,
        after: Option<(chrono::DateTime<chrono::Utc>, &str)>,
        fetch_limit: i64,
    ) -> Result<Vec<RouteSummary>, sqlx::Error> {
        type Row = (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            chrono::DateTime<chrono::Utc>,
        );

        let rows = if let Some((after_updated_at, after_id)) = after {
            sqlx::query_as::<_, Row>(
                r"
                SELECT r.id,
                       r.content_type,
                       r.title,
                       r.description,
                       r.owner_id,
                       latest.changed_at AS updated_at
                FROM routes r
                JOIN LATERAL (
                    SELECT h.changed_at
                    FROM pointer_history h
                    WHERE h.route_id = r.id
                    ORDER BY h.changed_at DESC, h.id DESC
                    LIMIT 1
                ) latest ON TRUE
                WHERE r.owner_id = $1
                  AND (latest.changed_at, r.id) < ($2, $3)
                ORDER BY latest.changed_at DESC, r.id DESC
                LIMIT $4
                ",
            )
            .bind(owner_id)
            .bind(after_updated_at)
            .bind(after_id)
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, Row>(
                r"
                SELECT r.id,
                       r.content_type,
                       r.title,
                       r.description,
                       r.owner_id,
                       latest.changed_at AS updated_at
                FROM routes r
                JOIN LATERAL (
                    SELECT h.changed_at
                    FROM pointer_history h
                    WHERE h.route_id = r.id
                    ORDER BY h.changed_at DESC, h.id DESC
                    LIMIT 1
                ) latest ON TRUE
                WHERE r.owner_id = $1
                ORDER BY latest.changed_at DESC, r.id DESC
                LIMIT $2
                ",
            )
            .bind(owner_id)
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows
            .into_iter()
            .map(
                |(id, content_type, title, description, owner_id, updated_at)| RouteSummary {
                    id,
                    annotations: RouteAnnotations {
                        content_type,
                        title,
                        description,
                    },
                    owner_id,
                    updated_at,
                },
            )
            .collect())
    }

    /// List a page of a route's version history, newest first.
    ///
    /// `after` is the `(changed_at, id)` keyset tuple of the last row the
    /// caller already has — `None` fetches the first (newest) page.
    /// `fetch_limit` should be the caller's page size *plus one*, matching the
    /// same "fetch one extra to detect more" convention as
    /// [`list_routes_for_owner_page`]. The append-only ledger is never
    /// truncated by this method — see [`Self::history_count`] for the exact
    /// total.
    pub async fn list_history_page(
        &self,
        id: &RouteId,
        after: Option<(chrono::DateTime<chrono::Utc>, i64)>,
        fetch_limit: i64,
    ) -> Result<Vec<HistoryEntry>, sqlx::Error> {
        type Row = (i64, String, String, chrono::DateTime<chrono::Utc>);

        let rows = if let Some((after_changed_at, after_id)) = after {
            sqlx::query_as::<_, Row>(
                r"
                SELECT id, target_hash, editor_id, changed_at
                FROM pointer_history
                WHERE route_id = $1
                  AND (changed_at, id) < ($2, $3)
                ORDER BY changed_at DESC, id DESC
                LIMIT $4
                ",
            )
            .bind(id.as_str())
            .bind(after_changed_at)
            .bind(after_id)
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, Row>(
                r"
                SELECT id, target_hash, editor_id, changed_at
                FROM pointer_history
                WHERE route_id = $1
                ORDER BY changed_at DESC, id DESC
                LIMIT $2
                ",
            )
            .bind(id.as_str())
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows
            .into_iter()
            .map(|(id, target_hash, editor_id, changed_at)| HistoryEntry {
                id,
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
            FROM pointer_history h
            JOIN content_blocks c ON c.hash_id = h.target_hash
            WHERE h.route_id = $1
              AND h.target_hash = $2
            LIMIT 1
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
        let history_id = sqlx::query_scalar::<_, i64>(
            r"
            WITH updated_route AS (
                UPDATE routes
                SET target_hash = $2
                WHERE id = $1
                  AND EXISTS (
                      SELECT 1
                      FROM pointer_history
                      WHERE route_id = $1
                        AND target_hash = $2
                  )
                RETURNING id
            )
            INSERT INTO pointer_history (route_id, target_hash, editor_id)
            SELECT id, $2, $3
            FROM updated_route
            RETURNING id
            ",
        )
        .bind(id.as_str())
        .bind(hash.as_str())
        .bind(editor_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(history_id.is_some())
    }
}
