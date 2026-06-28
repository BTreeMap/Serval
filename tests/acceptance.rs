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

use std::time::Duration;

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
        let cache = DeliveryCache::new(32 * 1024 * 1024, Duration::from_secs(300));
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
