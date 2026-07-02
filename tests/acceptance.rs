//! End-to-end integration tests asserting Serval's four acceptance criteria.
//!
//! These are gated behind the `integration` feature so the default
//! `cargo test` stays green on machines without Docker. Run them with:
//!
//! ```bash
//! cargo test --features integration --test acceptance
//! ```
//!
//! Each test boots an ephemeral PostgreSQL via `testcontainers`, runs both
//! planes in-process on loopback ports over a *single shared cache* — exactly
//! as the binary wires them — and drives them over real HTTP with `reqwest`.
#![cfg(feature = "integration")]

use serde_json::{Value, json};
use serval::api;
use serval::auth::{AuthConfig, AuthService};
use serval::cache::DeliveryCache;
use serval::crypto::IdSigner;
use serval::db::{self, Repository};
use serval::delivery;
use serval::state::{ControlState, DeliveryState};
use std::sync::Arc;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tokio::net::TcpListener;

/// Deployment secret used by the test harness to key the route-id MAC.
const TEST_ID_SECRET: &str = "acceptance-suite-id-signing-secret-please";

/// A running Serval under test: both plane base URLs plus the container guard
/// (dropping it tears down PostgreSQL).
struct Harness {
    control_base: String,
    data_base: String,
    client: reqwest::Client,
    signer: IdSigner,
    _pg: ContainerAsync<Postgres>,
}

impl Harness {
    /// Boot PostgreSQL, connect, and serve both planes over a shared cache.
    async fn start() -> Self {
        let pg = Postgres::default()
            .start()
            .await
            .expect("failed to start postgres container");
        let host = pg.get_host().await.expect("container host");
        let port = pg
            .get_host_port_ipv4(5432)
            .await
            .expect("container port mapping");
        let database_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

        let pool = db::connect(&database_url, 8)
            .await
            .expect("failed to connect and apply schema");
        let repo = Repository::new(pool);

        // The single shared cache is the crux of acceptance criterion #1.
        let cache = DeliveryCache::new(32 * 1024 * 1024);
        // One signer shared by both planes, exactly as the binary wires it.
        let signer = IdSigner::new(TEST_ID_SECRET);
        let auth = Arc::new(
            AuthService::new(AuthConfig::None)
                .await
                .expect("auth service"),
        );

        let control_state = ControlState {
            repo: repo.clone(),
            cache: cache.clone(),
            auth,
            signer: signer.clone(),
            data_plane_url: None,
        };
        let data_state = DeliveryState {
            repo,
            cache,
            signer: signer.clone(),
        };

        let control_base = serve(api::router(control_state)).await;
        let data_base = serve(delivery::router(data_state)).await;

        Self {
            control_base,
            data_base,
            client: reqwest::Client::new(),
            signer,
            _pg: pg,
        }
    }

    /// `POST /api/snippets`, returning the created route's JSON body.
    async fn create(&self, body: Value) -> Value {
        let resp = self
            .client
            .post(format!("{}/api/snippets", self.control_base))
            .json(&body)
            .send()
            .await
            .expect("create request");
        assert!(
            resp.status().is_success(),
            "create failed: {}",
            resp.status()
        );
        resp.json().await.expect("create json")
    }

    /// `PATCH /api/snippets/{id}` with new content; asserts success.
    async fn update(&self, id: &str, content: &str) {
        let resp = self
            .client
            .patch(format!("{}/api/snippets/{id}", self.control_base))
            .json(&json!({ "content": content }))
            .send()
            .await
            .expect("update request");
        assert!(
            resp.status().is_success(),
            "update failed: {}",
            resp.status()
        );
    }

    /// `PATCH /api/snippets/{id}` changing only the stored `content_type`.
    async fn update_content_type(&self, id: &str, content_type: &str) {
        let resp = self
            .client
            .patch(format!("{}/api/snippets/{id}", self.control_base))
            .json(&json!({ "content_type": content_type }))
            .send()
            .await
            .expect("content-type update request");
        assert!(
            resp.status().is_success(),
            "content-type update failed: {}",
            resp.status()
        );
    }

    /// `PATCH /api/snippets/{id}` updating only the `title` annotation.
    async fn update_title(&self, id: &str, title: &str) {
        let resp = self
            .client
            .patch(format!("{}/api/snippets/{id}", self.control_base))
            .json(&json!({ "title": title }))
            .send()
            .await
            .expect("title update request");
        assert!(
            resp.status().is_success(),
            "title update failed: {}",
            resp.status()
        );
    }

    /// `PATCH /api/snippets/{id}` updating only the `description` annotation.
    async fn update_description(&self, id: &str, description: &str) {
        let resp = self
            .client
            .patch(format!("{}/api/snippets/{id}", self.control_base))
            .json(&json!({ "description": description }))
            .send()
            .await
            .expect("description update request");
        assert!(
            resp.status().is_success(),
            "description update failed: {}",
            resp.status()
        );
    }

    /// `GET /api/snippets/{id}`, returning the detail JSON body.
    async fn detail(&self, id: &str) -> Value {
        let resp = self
            .client
            .get(format!("{}/api/snippets/{id}", self.control_base))
            .send()
            .await
            .expect("detail request");
        assert!(
            resp.status().is_success(),
            "detail failed: {}",
            resp.status()
        );
        resp.json().await.expect("detail json")
    }

    /// `GET /api/snippets/{id}/versions/{hash}`, returning the version JSON.
    async fn version(&self, id: &str, hash: &str) -> Value {
        let resp = self
            .client
            .get(format!(
                "{}/api/snippets/{id}/versions/{hash}",
                self.control_base
            ))
            .send()
            .await
            .expect("version request");
        assert!(
            resp.status().is_success(),
            "version failed: {}",
            resp.status()
        );
        resp.json().await.expect("version json")
    }

    /// `POST /api/snippets/{id}/restore` repointing the snippet to `hash`.
    async fn restore(&self, id: &str, hash: &str) {
        let resp = self
            .client
            .post(format!("{}/api/snippets/{id}/restore", self.control_base))
            .json(&json!({ "target_hash": hash }))
            .send()
            .await
            .expect("restore request");
        assert!(
            resp.status().is_success(),
            "restore failed: {}",
            resp.status()
        );
    }

    /// `GET /api/snippets` with optional query params, returning the raw
    /// JSON envelope (`{ snippets, next_cursor, limit }`).
    async fn list(&self, query: &[(&str, &str)]) -> Value {
        let resp = self
            .client
            .get(format!("{}/api/snippets", self.control_base))
            .query(query)
            .send()
            .await
            .expect("list request");
        assert!(resp.status().is_success(), "list failed: {}", resp.status());
        resp.json().await.expect("list json")
    }

    /// `GET /api/snippets` returning only the HTTP status, for asserting
    /// invalid `limit`/`cursor` values are rejected.
    async fn list_status(&self, query: &[(&str, &str)]) -> reqwest::StatusCode {
        self.client
            .get(format!("{}/api/snippets", self.control_base))
            .query(query)
            .send()
            .await
            .expect("list request")
            .status()
    }

    /// `GET /api/snippets/{id}/history` with optional query params, returning
    /// the raw JSON envelope (`{ history, next_cursor, limit }`).
    async fn history_page(&self, id: &str, query: &[(&str, &str)]) -> Value {
        let resp = self
            .client
            .get(format!("{}/api/snippets/{id}/history", self.control_base))
            .query(query)
            .send()
            .await
            .expect("history page request");
        assert!(
            resp.status().is_success(),
            "history page failed: {}",
            resp.status()
        );
        resp.json().await.expect("history page json")
    }

    /// `GET /api/snippets/{id}/history` returning only the HTTP status, for
    /// asserting invalid `limit`/`cursor` values are rejected.
    async fn history_status(&self, id: &str, query: &[(&str, &str)]) -> reqwest::StatusCode {
        self.client
            .get(format!("{}/api/snippets/{id}/history", self.control_base))
            .query(query)
            .send()
            .await
            .expect("history page request")
            .status()
    }

    /// `GET {data}/{path}` on the Data Plane, returning the body and headers.
    async fn deliver(&self, path: &str) -> (String, reqwest::header::HeaderMap) {
        let resp = self
            .client
            .get(format!("{}/{path}", self.data_base))
            .send()
            .await
            .expect("delivery request");
        assert!(
            resp.status().is_success(),
            "delivery failed: {}",
            resp.status()
        );
        let headers = resp.headers().clone();
        let body = resp.text().await.expect("delivery body");
        (body, headers)
    }

    /// `GET {data}/{path}` returning only the HTTP status, without asserting
    /// success — used to prove forged ids are rejected.
    async fn deliver_status(&self, path: &str) -> reqwest::StatusCode {
        self.client
            .get(format!("{}/{path}", self.data_base))
            .send()
            .await
            .expect("delivery request")
            .status()
    }

    /// `GET {data}/{path}` with `If-None-Match: <etag>`, returning the full
    /// response. Used to exercise the conditional-GET / 304 paths.
    async fn deliver_conditional(&self, path: &str, etag: &str) -> reqwest::Response {
        self.client
            .get(format!("{}/{path}", self.data_base))
            .header("If-None-Match", etag)
            .send()
            .await
            .expect("conditional delivery request")
    }
}

/// Bind an ephemeral loopback port and serve `router` on it in a task.
async fn serve(router: axum::Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    format!("http://{addr}")
}

/// Acceptance criterion #1: a Control Plane update is reflected on the very next
/// Data Plane GET, proving the shared cache was evicted across tasks.
#[tokio::test]
async fn cross_thread_invalidation() {
    let h = Harness::start().await;

    let created = h.create(json!({ "content": "version one" })).await;
    let id = created["id"].as_str().expect("id").to_owned();

    // Warm the Data Plane cache with v1.
    let (body, _) = h.deliver(&id).await;
    assert_eq!(body, "version one");

    // Update through the Control Plane.
    h.update(&id, "version two").await;

    // The very next read must reflect v2.
    let (body, _) = h.deliver(&id).await;
    assert_eq!(body, "version two", "cache was not evicted on update");
}

/// Acceptance criterion #2: known placeholders are substituted from the query
/// string while unknown ones survive verbatim.
#[tokio::test]
async fn tolerant_rendering() {
    let h = Harness::start().await;

    let created = h.create(json!({ "content": "{{uuid}} on {{port}}" })).await;
    let id = created["id"].as_str().expect("id").to_owned();

    let (body, _) = h.deliver(&format!("{id}?port=8080")).await;
    assert_eq!(body, "{{uuid}} on 8080");
}

/// Acceptance criterion #3: a version's content hash is the signed content id
/// `Base64URL(BLAKE3(content) || keyed-MAC)` — deterministic under the
/// deployment secret and extension-independent — and the Data Plane serves that
/// hash directly as an immutable version pointer. Permalinks are an internal
/// content-addressing detail: snippets themselves are created with random ids.
#[tokio::test]
async fn content_addressed_delivery() {
    let h = Harness::start().await;

    let content = "deterministic content";
    let expected = h.signer.content_id(content);

    // A snippet is created with an unguessable, random id — never the hash.
    let created = h.create(json!({ "content": content })).await;
    let id = created["id"].as_str().expect("id").to_owned();
    assert_eq!(id.len(), 64);
    assert_ne!(
        id, expected,
        "snippet ids must be random, not the content id"
    );

    // Its current version's hash IS the signed content id, and is itself a
    // valid, MAC-bearing id.
    let detail = h.detail(&id).await;
    let hash = detail["history"][0]["target_hash"]
        .as_str()
        .expect("target_hash")
        .to_owned();
    assert_eq!(hash, expected, "version hash is not the signed content id");
    assert_eq!(hash.len(), 64);
    assert!(
        h.signer.verify(&hash),
        "content hash must carry a valid MAC"
    );

    // The Data Plane serves that hash directly, immutably cached. A cosmetic
    // filename changes the served MIME but never the address.
    let (body, headers) = h.deliver(&format!("{hash}/snippet.json")).await;
    assert_eq!(body, content);
    assert_eq!(
        headers.get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    let cache_control = headers
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        cache_control.contains("immutable"),
        "a content-addressed version must be immutably cached, got {cache_control:?}"
    );
}

/// Restoring repoints a snippet back to an earlier version's content and records
/// the restore as a fresh ledger entry — history only ever grows.
#[tokio::test]
async fn restore_repoints_and_appends_history() {
    let h = Harness::start().await;

    let created = h.create(json!({ "content": "original" })).await;
    let id = created["id"].as_str().expect("id").to_owned();

    h.update(&id, "edited").await;

    // History is newest-first: index 0 is "edited", the last is "original".
    let detail = h.detail(&id).await;
    let history = detail["history"].as_array().expect("history");
    assert_eq!(history.len(), 2);
    let original_hash = history
        .last()
        .and_then(|e| e["target_hash"].as_str())
        .expect("original hash")
        .to_owned();

    // The earlier version's content is still retrievable verbatim.
    let version = h.version(&id, &original_hash).await;
    assert_eq!(version["content"], json!("original"));

    // Restore repoints the live snippet and the next Data Plane GET reflects it.
    h.restore(&id, &original_hash).await;
    let (body, _) = h.deliver(&id).await;
    assert_eq!(body, "original", "restore did not repoint the snippet");

    // The restore appended a third ledger row — nothing was pruned.
    let detail = h.detail(&id).await;
    assert_eq!(
        detail["history_count"].as_u64(),
        Some(3),
        "restore must append a new version, not rewrite history"
    );
}

/// DoS mitigation: the Data Plane rejects ids that do not carry a valid keyed
/// MAC — forged, enumerated, or signed by a different secret — with `404`,
/// before any cache or database lookup. A genuine alias still serves `200`.
#[tokio::test]
async fn forged_ids_are_rejected_by_the_data_plane() {
    let h = Harness::start().await;

    // A well-formed 64-char id whose prefix is attacker-chosen but whose MAC is
    // wrong (all 'A's): correct shape, invalid signature.
    let forged = "A".repeat(64);
    assert_eq!(
        h.deliver_status(&forged).await,
        reqwest::StatusCode::NOT_FOUND,
        "an id with an invalid MAC must be rejected"
    );

    // An id minted under a different secret must also fail verification here.
    let alien = IdSigner::new("a-totally-different-deployment-secret-xx").random_id();
    assert_eq!(
        h.deliver_status(&alien).await,
        reqwest::StatusCode::NOT_FOUND,
        "an id signed by another secret must be rejected"
    );

    // A genuine, signed alias is admitted and served.
    let created = h.create(json!({ "content": "genuine" })).await;
    let id = created["id"].as_str().expect("id");
    assert!(h.signer.verify(id), "harness must mint valid ids");
    let (body, _) = h.deliver(id).await;
    assert_eq!(body, "genuine");
}

/// Acceptance criterion #4: 100 updates to an alias yield exactly 101
/// `pointer_history` rows (1 create + 100 updates), with no pruning.
#[tokio::test]
async fn infinite_ledger() {
    let h = Harness::start().await;

    let created = h.create(json!({ "content": "v0" })).await;
    let id = created["id"].as_str().expect("id").to_owned();

    for i in 1..=100 {
        h.update(&id, &format!("v{i}")).await;
    }

    let detail = h.detail(&id).await;
    assert_eq!(
        detail["history_count"].as_u64(),
        Some(101),
        "ledger must retain every version"
    );
}

/// A `content_type`-only update is pure route metadata: it changes the stored
/// MIME, takes effect on the next Data Plane GET (cache evicted), and records no
/// new history row — the version ledger tracks content, not presentation.
#[tokio::test]
async fn content_type_update_is_metadata_only() {
    let h = Harness::start().await;

    let created = h.create(json!({ "content": "{ \"k\": 1 }" })).await;
    let id = created["id"].as_str().expect("id").to_owned();

    // Warm the Data Plane cache with the default text/plain type.
    let (_, headers) = h.deliver(&id).await;
    assert_eq!(
        headers.get("content-type").and_then(|v| v.to_str().ok()),
        Some("text/plain; charset=utf-8"),
        "default stored content type should serve verbatim"
    );

    // Repoint only the content type — no content field.
    h.update_content_type(&id, "application/json").await;

    // The metadata is reflected on the route and on the very next read.
    let detail = h.detail(&id).await;
    assert_eq!(detail["content_type"], json!("application/json"));
    assert_eq!(
        detail["history_count"].as_u64(),
        Some(1),
        "changing content type must not append a version"
    );

    let (_, headers) = h.deliver(&id).await;
    assert_eq!(
        headers.get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/json"),
        "cache was not evicted on a content-type update"
    );
}

/// Title and description are un-historied annotations on the route. Setting,
/// updating, and clearing them is reflected on the next detail fetch and on
/// the listing, without ever appending a new row to the version ledger.
#[tokio::test]
async fn title_and_description_are_metadata_only() {
    let h = Harness::start().await;

    // Create without annotations; both fields should be absent.
    let created = h.create(json!({ "content": "data" })).await;
    let id = created["id"].as_str().expect("id").to_owned();
    assert!(
        created["title"].is_null(),
        "title should be null on creation"
    );
    assert!(
        created["description"].is_null(),
        "description should be null on creation"
    );

    // Set title and description; detail and list should reflect them.
    h.update_title(&id, "My Snippet").await;
    h.update_description(&id, "A test snippet").await;

    let detail = h.detail(&id).await;
    assert_eq!(detail["title"], json!("My Snippet"));
    assert_eq!(detail["description"], json!("A test snippet"));
    assert_eq!(
        detail["history_count"].as_u64(),
        Some(1),
        "annotation updates must not append a version"
    );

    // Create with title+description inline at creation time.
    let created2 = h
        .create(json!({
            "content": "v1",
            "title": "Inline Title",
            "description": "  Trimmed  "
        }))
        .await;
    assert_eq!(created2["title"], json!("Inline Title"));
    assert_eq!(
        created2["description"],
        json!("Trimmed"),
        "leading/trailing whitespace should be stripped"
    );

    // Updating title to empty string clears it (sets to null).
    let id2 = created2["id"].as_str().expect("id2").to_owned();
    h.update_title(&id2, "").await;
    let detail2 = h.detail(&id2).await;
    assert!(
        detail2["title"].is_null(),
        "empty title update should clear the annotation"
    );

    // Description remains unchanged after clearing only the title.
    assert_eq!(detail2["description"], json!("Trimmed"));
    assert_eq!(
        detail2["history_count"].as_u64(),
        Some(1),
        "clearing a title must not append a version"
    );
}

/// Every `200` delivery response carries a strong `ETag` header, and a repeat
/// request with `If-None-Match` set to that value returns `304 Not Modified`
/// with no body.
#[tokio::test]
async fn etag_and_conditional_get() {
    let h = Harness::start().await;

    let created = h.create(json!({ "content": "etag payload" })).await;
    let id = created["id"].as_str().expect("id").to_owned();

    // First GET: must carry an ETag.
    let (_, headers) = h.deliver(&id).await;
    let etag = headers
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("ETag header must be present on 200")
        .to_owned();
    assert!(
        etag.starts_with('"') && etag.ends_with('"'),
        "ETag must be a double-quoted strong validator, got {etag:?}"
    );

    // Second GET with If-None-Match: must return 304 with no body.
    let resp = h.deliver_conditional(&id, &etag).await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_MODIFIED,
        "matching ETag must yield 304"
    );
    let body = resp.bytes().await.expect("304 bytes");
    assert!(body.is_empty(), "304 must have no body");
}

/// After a content update, the old ETag is stale: a conditional GET with it
/// returns `200` with the new body and a new ETag.
#[tokio::test]
async fn conditional_get_reflects_update() {
    let h = Harness::start().await;

    let created = h.create(json!({ "content": "before" })).await;
    let id = created["id"].as_str().expect("id").to_owned();

    let (_, headers) = h.deliver(&id).await;
    let old_etag = headers
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("ETag on first 200")
        .to_owned();

    h.update(&id, "after").await;

    // INM with the old ETag must yield 200 with new content.
    let resp = h.deliver_conditional(&id, &old_etag).await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "stale ETag must not 304 after an update"
    );
    let new_headers = resp.headers().clone();
    let body = resp.text().await.expect("200 body");
    assert_eq!(body, "after");

    let new_etag = new_headers
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("ETag on second 200")
        .to_owned();
    assert_ne!(old_etag, new_etag, "ETag must change when content changes");
}

/// Content-addressed (immutable) ids emit `ETag: "<id>"` and any conditional
/// GET with that value returns `304` immediately — no cache or DB work needed.
#[tokio::test]
async fn immutable_etag_is_id_and_304_is_instant() {
    let h = Harness::start().await;

    let content = "immutable content for etag test";
    let hash = h.signer.content_id(content);

    // Populate the block via the normal creation path.
    h.create(json!({ "content": content })).await;

    // Serve the content-addressed version to get its ETag.
    let (_, headers) = h.deliver(&hash).await;
    let etag = headers
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("ETag on immutable 200")
        .to_owned();

    // The ETag for an immutable id is `"<id>"` — the id itself double-quoted.
    assert_eq!(
        etag,
        format!("\"{}\"", hash),
        "immutable ETag must equal the double-quoted id"
    );

    // Conditional GET must return 304.
    let resp = h.deliver_conditional(&hash, &etag).await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_MODIFIED,
        "immutable INM shortcut must yield 304"
    );
}

/// `Cache-Control` for mutable routes is `no-cache`: a downstream cache may
/// store the response but must revalidate via the strong ETag before reuse,
/// because in-process invalidation cannot reach the edge.
#[tokio::test]
async fn cache_control_mutable_is_no_cache() {
    let h = Harness::start().await;

    let created = h.create(json!({ "content": "no-cache payload" })).await;
    let id = created["id"].as_str().expect("id").to_owned();

    let (_, headers) = h.deliver(&id).await;
    let cc = headers
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    assert_eq!(
        cc, "no-cache",
        "mutable route must carry no-cache in Cache-Control, got {cc:?}"
    );
}

/// `GET /api/snippets` returns pages of at most the requested `limit`, and
/// walking `next_cursor` to the end visits every created route exactly once,
/// with no duplicates or gaps — the keyset invariant a cursor must uphold.
#[tokio::test]
async fn list_snippets_pages_without_overlap() {
    let h = Harness::start().await;

    let mut ids = Vec::new();
    for i in 0..5 {
        let created = h.create(json!({ "content": format!("snippet {i}") })).await;
        ids.push(created["id"].as_str().expect("id").to_owned());
    }

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut query = vec![("limit", "2")];
        if let Some(c) = cursor.as_deref() {
            query.push(("cursor", c));
        }
        let page = h.list(&query).await;
        let snippets = page["snippets"].as_array().expect("snippets");
        assert!(snippets.len() <= 2, "page must respect limit");
        for s in snippets {
            seen.push(s["id"].as_str().expect("id").to_owned());
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }

    seen.sort();
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(
        seen, expected,
        "cursor traversal must visit every route exactly once"
    );
}

/// Invalid `limit` and `cursor` values on `GET /api/snippets` are rejected
/// with `400`, never silently clamped or ignored.
#[tokio::test]
async fn list_snippets_rejects_invalid_limit_and_cursor() {
    let h = Harness::start().await;

    assert_eq!(
        h.list_status(&[("limit", "0")]).await,
        reqwest::StatusCode::BAD_REQUEST,
        "limit=0 must be rejected"
    );
    assert_eq!(
        h.list_status(&[("limit", "51")]).await,
        reqwest::StatusCode::BAD_REQUEST,
        "limit over the cap must be rejected"
    );
    assert_eq!(
        h.list_status(&[("limit", "not-a-number")]).await,
        reqwest::StatusCode::BAD_REQUEST,
        "non-numeric limit must be rejected"
    );
    assert_eq!(
        h.list_status(&[("cursor", "not a valid cursor")]).await,
        reqwest::StatusCode::BAD_REQUEST,
        "malformed cursor must be rejected"
    );
}

/// `GET /api/snippets/{id}` caps its embedded history page at `history_limit`,
/// but `history_count` always reports the exact ledger total — proving the
/// two numbers are computed independently rather than one deriving from the
/// length of the returned page.
#[tokio::test]
async fn snippet_detail_caps_history_page_but_reports_exact_count() {
    let h = Harness::start().await;

    let created = h.create(json!({ "content": "v0" })).await;
    let id = created["id"].as_str().expect("id").to_owned();
    for i in 1..=12 {
        h.update(&id, &format!("v{i}")).await;
    }

    let resp = h
        .client
        .get(format!("{}/api/snippets/{id}?limit=5", h.control_base))
        .send()
        .await
        .expect("detail request");
    assert!(resp.status().is_success());
    let detail: Value = resp.json().await.expect("detail json");

    assert_eq!(detail["history_count"].as_u64(), Some(13));
    let history = detail["history"].as_array().expect("history");
    assert_eq!(history.len(), 5, "embedded history page must respect limit");
    assert!(
        detail["history_next_cursor"].as_str().is_some(),
        "more history remains, so a next_cursor must be present"
    );
    assert_eq!(history[0]["is_current"], json!(true));
    assert_eq!(history[0]["version_number"], json!(13));
}

/// Paginating `GET /api/snippets/{id}/history` to the end visits every one of
/// 101 ledger entries exactly once, and each entry's `version_number` stays
/// stable and monotonically decreasing across page boundaries — the whole
/// point of carrying `snapshot_total`/`loaded_count` inside the cursor.
#[tokio::test]
async fn history_pagination_covers_full_ledger_with_stable_version_numbers() {
    let h = Harness::start().await;

    let created = h.create(json!({ "content": "v0" })).await;
    let id = created["id"].as_str().expect("id").to_owned();
    for i in 1..=100 {
        h.update(&id, &format!("v{i}")).await;
    }

    // First page comes from the detail endpoint.
    let detail = h.detail(&id).await;
    assert_eq!(detail["history_count"].as_u64(), Some(101));
    let mut version_numbers = Vec::new();
    for item in detail["history"].as_array().expect("history") {
        version_numbers.push(item["version_number"].as_i64().expect("version_number"));
    }
    assert_eq!(version_numbers[0], 101, "newest entry is version 101");
    assert_eq!(detail["history"][0]["is_current"], json!(true));

    // Walk the rest via the dedicated history endpoint.
    let mut cursor = detail["history_next_cursor"]
        .as_str()
        .map(str::to_owned)
        .expect("more history to page through");
    loop {
        let page = h.history_page(&id, &[("cursor", &cursor)]).await;
        for item in page["history"].as_array().expect("history") {
            assert_eq!(
                item["is_current"],
                json!(false),
                "only the newest entry may be current"
            );
            version_numbers.push(item["version_number"].as_i64().expect("version_number"));
        }
        match page["next_cursor"].as_str() {
            Some(next) => cursor = next.to_owned(),
            None => break,
        }
    }

    assert_eq!(
        version_numbers.len(),
        101,
        "every ledger row must be visited exactly once"
    );
    let expected: Vec<i64> = (1..=101).rev().collect();
    assert_eq!(
        version_numbers, expected,
        "version numbers must be stable and strictly decreasing across pages"
    );
}

/// Invalid `limit` and `cursor` values on `GET /api/snippets/{id}/history` are
/// rejected with `400`, and a cursor minted for the snippet-list endpoint is
/// rejected as a type mismatch rather than silently misinterpreted.
#[tokio::test]
async fn history_page_rejects_invalid_limit_and_cursor() {
    let h = Harness::start().await;

    let created = h.create(json!({ "content": "v0" })).await;
    let id = created["id"].as_str().expect("id").to_owned();
    // A second route ensures the list endpoint actually has a next page to
    // mint a cursor from, so the cross-endpoint rejection below is exercised.
    h.create(json!({ "content": "other snippet" })).await;

    assert_eq!(
        h.history_status(&id, &[("limit", "0")]).await,
        reqwest::StatusCode::BAD_REQUEST,
        "limit=0 must be rejected"
    );
    assert_eq!(
        h.history_status(&id, &[("limit", "51")]).await,
        reqwest::StatusCode::BAD_REQUEST,
        "limit over the cap must be rejected"
    );
    assert_eq!(
        h.history_status(&id, &[("cursor", "garbage")]).await,
        reqwest::StatusCode::BAD_REQUEST,
        "malformed cursor must be rejected"
    );

    // A cursor minted for the snippet list must not type-check here.
    let list_page = h.list(&[("limit", "1")]).await;
    let list_cursor = list_page["next_cursor"]
        .as_str()
        .expect("two routes exist, so the list page must have a next_cursor");
    assert_eq!(
        h.history_status(&id, &[("cursor", list_cursor)]).await,
        reqwest::StatusCode::BAD_REQUEST,
        "a snippets-list cursor must not be accepted by the history endpoint"
    );
}
