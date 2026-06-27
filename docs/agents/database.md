# Database & Migration Integrity

**Read this before any change that touches persistence, models, or queries.**

Data integrity is the one place where "ruthless refactoring / no backward
compatibility" does **not** apply. You may freely rewrite Rust interfaces, but
you must **never** put existing data at risk. Every schema-affecting code change
ships a correct migration.

Serval is **PostgreSQL-exclusive** for both development and production. There is
no SQLite path to mirror.

## The CAS data model

Three tables separate heavy content, active routing, and the audit trail.

### `content_blocks` — the immutable blob layer

| Column | Type | Constraints | Notes |
|---|---|---|---|
| `hash_id` | `VARCHAR(64)` | PRIMARY KEY | Signed content id: `Base64URL(BLAKE3(content) \|\| keyed-MAC)`, 64 chars |
| `content` | `TEXT` | NOT NULL | The heavy 20KB+ payload |

Pure deduplication: identical content is stored exactly once. Blocks are
**write-once** — inserted with `ON CONFLICT DO NOTHING`, never updated or
deleted. The `hash_id` is itself a valid, MAC-signed route id — a content
address is directly servable as its immutable permalink.

### `routes` — the active routing layer

| Column | Type | Constraints | Notes |
|---|---|---|---|
| `id` | `VARCHAR(64)` | PRIMARY KEY | 64-char signed id: 32-byte prefix + 16-byte keyed MAC (alias or permalink) |
| `target_hash` | `VARCHAR(64)` | FK → `content_blocks` | Pointer to current payload |
| `content_type` | `VARCHAR(255)` | NOT NULL | Default `text/plain; charset=utf-8` |
| `cache_mode` | `SMALLINT` | NOT NULL | `0` = mutable (short TTL), `1` = immutable (edge-cached) |
| `owner_id` | `VARCHAR(255)` | NULL | Authenticated creator |

One id format covers both kinds: every id is `prefix || MAC`, where `MAC =
BLAKE3::keyed_hash(key, prefix)` truncated to 16 bytes and `key` is derived from
the deployment-wide `ID_SIGNING_SECRET`. For an **immutable permalink** the
prefix is `BLAKE3(content)`, so `id == target_hash` — the content hash itself.
For a **mutable alias** the prefix is 32 CSPRNG bytes. The MAC is recomputed
and verified on every Data Plane read (see
[delivery.md](delivery.md)); it is never stored.

### `pointer_history` — the append-only version ledger

| Column | Type | Constraints | Notes |
|---|---|---|---|
| `id` | `SERIAL` | PRIMARY KEY | Internal ledger id |
| `route_id` | `VARCHAR(64)` | FK → `routes` | The mutable link updated |
| `target_hash` | `VARCHAR(64)` | FK → `content_blocks` | Content hash at this point in time |
| `editor_id` | `VARCHAR(255)` | NOT NULL | Authenticated user who made the change |
| `changed_at` | `TIMESTAMPTZ` | DEFAULT NOW() | Timestamp of the edit |

**Infinite and append-only.** No pruning, truncation, or retention cap is ever
applied. Creating a route appends version 1; each `PATCH` appends one row.

## Rules for changing the schema

1. **Idempotent at startup.** Schema creation must be safe to run on every boot:
   use `CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`, and guarded
   `ALTER TABLE ... ADD COLUMN`.
2. **Additive only.** New columns/tables/indexes must be additive. Never drop or
   rename a populated column/table without a data-preserving copy step.
3. **Backfill, don't break.** New non-null columns need a default or a backfill
   so existing rows remain valid after upgrade.
4. **Preserve the core invariants:**
   - `content_blocks` stays immutable and content-addressed.
   - `pointer_history` stays append-only with no pruning.
   - Permalink `id` stays equal to the signed content id
     `Base64URL(BLAKE3(content) || keyed-MAC)`.
5. **Validate on a live PostgreSQL 16+ instance** via the Dockerized
   integration suite (see [testing.md](testing.md)) before declaring done.

## Decision table

| You are… | Required action |
|---|---|
| Adding a column | Additive + defaulted in startup schema; backfill existing rows |
| Adding a table/index | `CREATE ... IF NOT EXISTS` in the startup schema |
| Renaming a field in Rust only | No migration needed; keep the DB column name or do a guarded copy |
| Renaming/removing a DB column | Copy data to the new shape first; never drop populated columns destructively |
| Changing history retention | Not allowed — `pointer_history` is infinite by design |
| Mutating stored content | Not allowed — write a new block; update the `routes` pointer |
