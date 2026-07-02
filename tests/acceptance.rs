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

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use serval::api;
use serval::auth::{AuthConfig, AuthService, OAuthSettings};
use serval::cache::DeliveryCache;
use serval::crypto::IdSigner;
use serval::db::{self, Repository};
use serval::delivery;
use serval::state::{ControlState, DeliveryState};
use std::sync::Arc;
use std::time::Duration;
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

    /// A raw Control Plane request, returning only the status — used to
    /// assert 4xx/404 boundaries without requiring a decodable success body.
    async fn raw_status(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> reqwest::StatusCode {
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.control_base));
        if let Some(b) = body {
            req = req.json(b);
        }
        req.send().await.expect("request").status()
    }

    /// `POST /api/snippets` returning only the status, for asserting
    /// create-time validation failures without needing a success body.
    async fn create_status(&self, body: Value) -> reqwest::StatusCode {
        self.client
            .post(format!("{}/api/snippets", self.control_base))
            .json(&body)
            .send()
            .await
            .expect("create request")
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

/// Serve a minimal JWKS document over HTTP containing one symmetric (`oct`)
/// key, so [`AuthService::new`]'s real JWKS fetch has something genuine to
/// validate against — no RSA keypair or `use_pem` machinery needed, since the
/// JWKS parser already supports `"kty": "oct"` for exactly this purpose.
async fn start_jwks_server(secret: &[u8], kid: &str) -> String {
    let jwks = json!({
        "keys": [{
            "kty": "oct",
            "kid": kid,
            "k": STANDARD.encode(secret),
        }]
    });
    let router = axum::Router::new().route(
        "/jwks.json",
        axum::routing::get(move || {
            let jwks = jwks.clone();
            async move { axum::Json(jwks) }
        }),
    );
    format!("{}/jwks.json", serve(router).await)
}

/// A running Serval under test wired for real `AuthMode::Oauth` enforcement,
/// backed by a self-hosted JWKS server — exercising the ownership/admin
/// authorization boundary end-to-end, which `AuthConfig::None` (a fixed
/// superuser identity) cannot reach at all.
struct OAuthHarness {
    control_base: String,
    client: reqwest::Client,
    repo: Repository,
    jwt_secret: Vec<u8>,
    kid: String,
    issuer: String,
    audience: String,
    _pg: ContainerAsync<Postgres>,
}

impl OAuthHarness {
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

        let jwt_secret = b"acceptance-suite-oauth-shared-secret".to_vec();
        let kid = "acceptance-test-key".to_owned();
        let issuer = "https://issuer.example.test".to_owned();
        let audience = "serval-acceptance".to_owned();
        let jwks_url = start_jwks_server(&jwt_secret, &kid).await;

        let auth = Arc::new(
            AuthService::new(AuthConfig::Oauth(OAuthSettings {
                issuer: issuer.clone(),
                audience: audience.clone(),
                jwks_url,
                jwks_cache_ttl: Duration::from_secs(300),
                client_id: "test-client".to_owned(),
                scopes: "openid".to_owned(),
                redirect_uri: "http://localhost/callback".to_owned(),
            }))
            .await
            .expect("oauth auth service"),
        );

        let cache = DeliveryCache::new(32 * 1024 * 1024);
        let signer = IdSigner::new(TEST_ID_SECRET);
        let control_state = ControlState {
            repo: repo.clone(),
            cache,
            auth,
            signer,
            data_plane_url: None,
        };
        let control_base = serve(api::router(control_state)).await;

        Self {
            control_base,
            client: reqwest::Client::new(),
            repo,
            jwt_secret,
            kid,
            issuer,
            audience,
            _pg: pg,
        }
    }

    /// Mint a valid, freshly-expiring JWT for `sub`.
    fn token(&self, sub: &str) -> String {
        self.token_with_exp(sub, chrono::Utc::now() + chrono::Duration::hours(1))
    }

    /// Mint a JWT for `sub` with an explicit expiry, for testing the expired-
    /// token boundary.
    fn token_with_exp(&self, sub: &str, exp: chrono::DateTime<chrono::Utc>) -> String {
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(self.kid.clone());
        let claims = json!({
            "sub": sub,
            "iss": self.issuer,
            "aud": self.audience,
            "exp": exp.timestamp(),
        });
        encode(
            &header,
            &claims,
            &EncodingKey::from_secret(&self.jwt_secret),
        )
        .expect("mint jwt")
    }

    /// `POST /api/snippets` authenticated as `sub`; asserts success.
    async fn create_as(&self, sub: &str, body: Value) -> Value {
        let resp = self
            .client
            .post(format!("{}/api/snippets", self.control_base))
            .bearer_auth(self.token(sub))
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

    /// A raw Control Plane request authenticated as `sub` (or unauthenticated
    /// if `None`), returning only the status.
    async fn status_as(
        &self,
        sub: Option<&str>,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> reqwest::StatusCode {
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.control_base));
        if let Some(s) = sub {
            req = req.bearer_auth(self.token(s));
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        req.send().await.expect("request").status()
    }
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

// ---------------------------------------------------------------------------
// Authorization boundary (requires real JWT identities; `AuthConfig::None`
// always yields one fixed superuser and so cannot reach this boundary at all).
// ---------------------------------------------------------------------------

/// Ownership is a hard authorization boundary: a non-owner, non-admin caller
/// must be rejected — with `403`, not `404` (existence is not hidden from a
/// stranger, only access is) — from every read and write on someone else's
/// snippet, and their own listing must never surface it.
#[tokio::test]
async fn ownership_forbids_cross_user_access() {
    let h = OAuthHarness::start().await;

    let created = h
        .create_as("alice", json!({ "content": "alice's secret" }))
        .await;
    let id = created["id"].as_str().expect("id").to_owned();

    let cases: Vec<(reqwest::Method, String, Option<Value>)> = vec![
        (reqwest::Method::GET, format!("/api/snippets/{id}"), None),
        (
            reqwest::Method::PATCH,
            format!("/api/snippets/{id}"),
            Some(json!({ "content": "hijacked" })),
        ),
        (
            reqwest::Method::GET,
            format!("/api/snippets/{id}/history"),
            None,
        ),
        (
            reqwest::Method::GET,
            format!("/api/snippets/{id}/versions/{}", "A".repeat(64)),
            None,
        ),
        (
            reqwest::Method::POST,
            format!("/api/snippets/{id}/restore"),
            Some(json!({ "target_hash": "A".repeat(64) })),
        ),
    ];
    for (method, path, body) in cases {
        let status = h
            .status_as(Some("bob"), method.clone(), &path, body.as_ref())
            .await;
        assert_eq!(
            status,
            reqwest::StatusCode::FORBIDDEN,
            "bob must be forbidden from {method} {path} on alice's snippet, got {status}"
        );
    }

    // Bob's own listing must not surface alice's snippet.
    let resp = h
        .client
        .get(format!("{}/api/snippets", h.control_base))
        .bearer_auth(h.token("bob"))
        .send()
        .await
        .expect("bob's list request");
    assert!(resp.status().is_success());
    let page: Value = resp.json().await.expect("list json");
    assert_eq!(
        page["snippets"].as_array().expect("snippets").len(),
        0,
        "bob must not see alice's snippets in his own listing"
    );
}

/// The admin role is a global escape hatch: once granted (via the local users
/// table, the out-of-band mechanism documented on `Repository::set_admin`), a
/// caller may read and write any user's snippet, bypassing ownership.
#[tokio::test]
async fn admin_role_grants_cross_user_access() {
    let h = OAuthHarness::start().await;

    let created = h
        .create_as("alice", json!({ "content": "alice's content" }))
        .await;
    let id = created["id"].as_str().expect("id").to_owned();

    // Bob is an ordinary stranger until promoted.
    assert_eq!(
        h.status_as(
            Some("bob"),
            reqwest::Method::GET,
            &format!("/api/snippets/{id}"),
            None
        )
        .await,
        reqwest::StatusCode::FORBIDDEN,
        "bob must be forbidden before promotion"
    );

    h.repo.set_admin("bob", true).await.expect("grant admin");

    assert_eq!(
        h.status_as(
            Some("bob"),
            reqwest::Method::GET,
            &format!("/api/snippets/{id}"),
            None
        )
        .await,
        reqwest::StatusCode::OK,
        "an admin must be able to read another user's snippet"
    );
    assert_eq!(
        h.status_as(
            Some("bob"),
            reqwest::Method::PATCH,
            &format!("/api/snippets/{id}"),
            Some(&json!({ "content": "admin edit" })),
        )
        .await,
        reqwest::StatusCode::OK,
        "an admin must be able to write another user's snippet"
    );
}

/// Every way a bearer token can fail to authenticate is rejected with `401`,
/// never a `500` or a silent pass-through: absent credentials, a non-bearer
/// scheme, an expired token, a token for the wrong audience, and a token whose
/// `kid` is not present in the JWKS (even after the forced-refresh retry).
#[tokio::test]
async fn invalid_or_expired_tokens_are_rejected() {
    let h = OAuthHarness::start().await;
    let me = format!("{}/api/me", h.control_base);

    let resp = h.client.get(&me).send().await.expect("no-auth request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "missing credentials must be rejected"
    );

    let resp = h
        .client
        .get(&me)
        .header("Authorization", "Basic dXNlcjpwYXNz")
        .send()
        .await
        .expect("non-bearer request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a non-bearer scheme must be rejected"
    );

    let expired = h.token_with_exp("alice", chrono::Utc::now() - chrono::Duration::hours(1));
    let resp = h
        .client
        .get(&me)
        .bearer_auth(expired)
        .send()
        .await
        .expect("expired-token request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "an expired token must be rejected"
    );

    let mut wrong_aud_header = Header::new(Algorithm::HS256);
    wrong_aud_header.kid = Some(h.kid.clone());
    let wrong_aud_claims = json!({
        "sub": "alice",
        "iss": h.issuer,
        "aud": "some-other-audience",
        "exp": (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp(),
    });
    let wrong_aud = encode(
        &wrong_aud_header,
        &wrong_aud_claims,
        &EncodingKey::from_secret(&h.jwt_secret),
    )
    .expect("mint wrong-audience jwt");
    let resp = h
        .client
        .get(&me)
        .bearer_auth(wrong_aud)
        .send()
        .await
        .expect("wrong-audience request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a token for a different audience must be rejected"
    );

    let mut unknown_kid_header = Header::new(Algorithm::HS256);
    unknown_kid_header.kid = Some("no-such-key".to_owned());
    let unknown_kid_claims = json!({
        "sub": "alice",
        "iss": h.issuer,
        "aud": h.audience,
        "exp": (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp(),
    });
    let unknown_kid = encode(
        &unknown_kid_header,
        &unknown_kid_claims,
        &EncodingKey::from_secret(&h.jwt_secret),
    )
    .expect("mint unknown-kid jwt");
    let resp = h
        .client
        .get(&me)
        .bearer_auth(unknown_kid)
        .send()
        .await
        .expect("unknown-kid request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a token with an unrecognized key id must be rejected"
    );

    // Sanity check: a genuinely valid token still authenticates.
    let resp = h
        .client
        .get(&me)
        .bearer_auth(h.token("alice"))
        .send()
        .await
        .expect("valid-token request");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let me_body: Value = resp.json().await.expect("me json");
    assert_eq!(me_body["user_id"], json!("alice"));
    assert_eq!(me_body["is_admin"], json!(false));
}

// ---------------------------------------------------------------------------
// Not-found / bad-request boundaries.
// ---------------------------------------------------------------------------

/// Every read/write under `/api/snippets/{id}` on a well-formed but
/// nonexistent id must `404` — never a `500`, and never silently treated as if
/// the route existed.
#[tokio::test]
async fn not_found_sweep_for_nonexistent_route() {
    let h = Harness::start().await;
    let nonexistent = h.signer.random_id();
    let some_hash = h.signer.content_id("irrelevant content");

    let cases: Vec<(reqwest::Method, String, Option<Value>)> = vec![
        (
            reqwest::Method::GET,
            format!("/api/snippets/{nonexistent}"),
            None,
        ),
        (
            reqwest::Method::PATCH,
            format!("/api/snippets/{nonexistent}"),
            Some(json!({ "content": "x" })),
        ),
        (
            reqwest::Method::GET,
            format!("/api/snippets/{nonexistent}/history"),
            None,
        ),
        (
            reqwest::Method::GET,
            format!("/api/snippets/{nonexistent}/versions/{some_hash}"),
            None,
        ),
        (
            reqwest::Method::POST,
            format!("/api/snippets/{nonexistent}/restore"),
            Some(json!({ "target_hash": some_hash })),
        ),
    ];
    for (method, path, body) in cases {
        let status = h.raw_status(method.clone(), &path, body.as_ref()).await;
        assert_eq!(
            status,
            reqwest::StatusCode::NOT_FOUND,
            "{method} {path} on a nonexistent route must 404, got {status}"
        );
    }
}

/// A structurally invalid route id (wrong length or illegal characters) is
/// rejected at the validation boundary with `400`, distinct from the `404` a
/// well-formed-but-unknown id receives — the client can tell "malformed
/// request" from "not found" apart.
#[tokio::test]
async fn malformed_ids_are_bad_request_not_not_found() {
    let h = Harness::start().await;
    let too_short = "abc";
    let illegal_chars = "!".repeat(64);
    let valid_hash = h.signer.content_id("whatever");

    for bad in [too_short, illegal_chars.as_str()] {
        assert_eq!(
            h.raw_status(reqwest::Method::GET, &format!("/api/snippets/{bad}"), None)
                .await,
            reqwest::StatusCode::BAD_REQUEST,
            "malformed route id {bad:?} must 400, not 404"
        );
        assert_eq!(
            h.raw_status(
                reqwest::Method::GET,
                &format!("/api/snippets/{bad}/versions/{valid_hash}"),
                None
            )
            .await,
            reqwest::StatusCode::BAD_REQUEST,
            "malformed route id {bad:?} in a version fetch must 400"
        );
    }

    // A well-formed route id but a malformed hash segment/body field.
    let created = h.create(json!({ "content": "v0" })).await;
    let id = created["id"].as_str().expect("id");
    assert_eq!(
        h.raw_status(
            reqwest::Method::GET,
            &format!("/api/snippets/{id}/versions/{too_short}"),
            None
        )
        .await,
        reqwest::StatusCode::BAD_REQUEST,
        "malformed hash in a version fetch must 400"
    );
    assert_eq!(
        h.raw_status(
            reqwest::Method::POST,
            &format!("/api/snippets/{id}/restore"),
            Some(&json!({ "target_hash": too_short })),
        )
        .await,
        reqwest::StatusCode::BAD_REQUEST,
        "malformed target_hash in a restore body must 400"
    );
}

/// A hash that is a genuine version of one route must never be readable or
/// restorable through a *different* route's id — the CAS layer deduplicates
/// storage, but the ledger's `route_id` scoping must still isolate routes.
#[tokio::test]
async fn version_and_restore_are_scoped_to_their_own_route() {
    let h = Harness::start().await;

    let a = h.create(json!({ "content": "route A v0" })).await;
    let a_id = a["id"].as_str().expect("id").to_owned();
    h.update(&a_id, "route A v1").await;
    let a_detail = h.detail(&a_id).await;
    let a_original_hash = a_detail["history"][1]["target_hash"]
        .as_str()
        .expect("original hash")
        .to_owned();

    let b = h.create(json!({ "content": "route B v0" })).await;
    let b_id = b["id"].as_str().expect("id").to_owned();

    // The hash genuinely is a version of route A.
    let v = h.version(&a_id, &a_original_hash).await;
    assert_eq!(v["content"], json!("route A v0"));

    // But reading it through route B's id must 404 — it is not B's version.
    assert_eq!(
        h.raw_status(
            reqwest::Method::GET,
            &format!("/api/snippets/{b_id}/versions/{a_original_hash}"),
            None
        )
        .await,
        reqwest::StatusCode::NOT_FOUND,
        "a hash belonging to another route must not be readable through this route's id"
    );

    // And restoring B to it must be rejected as not-a-version-of-this-route.
    assert_eq!(
        h.raw_status(
            reqwest::Method::POST,
            &format!("/api/snippets/{b_id}/restore"),
            Some(&json!({ "target_hash": a_original_hash })),
        )
        .await,
        reqwest::StatusCode::BAD_REQUEST,
        "restoring to a hash that is not a genuine version of this route must be rejected"
    );
}

/// A partial update must change something: a `PATCH` body with no recognized
/// fields is rejected with `400` rather than silently succeeding as a no-op.
#[tokio::test]
async fn update_requires_at_least_one_field() {
    let h = Harness::start().await;
    let created = h.create(json!({ "content": "v0" })).await;
    let id = created["id"].as_str().expect("id").to_owned();

    assert_eq!(
        h.raw_status(
            reqwest::Method::PATCH,
            &format!("/api/snippets/{id}"),
            Some(&json!({})),
        )
        .await,
        reqwest::StatusCode::BAD_REQUEST,
        "an update with no fields must be rejected"
    );
}

/// Metadata validation (overlong or illegal-header `content_type`; overlong
/// `title`/`description`) is enforced identically at creation and at update,
/// and the exact length boundary is accepted rather than off-by-one rejected.
#[tokio::test]
async fn create_and_update_reject_invalid_metadata() {
    let h = Harness::start().await;

    assert_eq!(
        h.create_status(json!({ "content": "x", "content_type": "bad\nvalue" }))
            .await,
        reqwest::StatusCode::BAD_REQUEST,
        "a content_type with illegal header bytes must be rejected at create"
    );
    assert_eq!(
        h.create_status(json!({ "content": "x", "content_type": "a".repeat(256) }))
            .await,
        reqwest::StatusCode::BAD_REQUEST,
        "an overlong content_type must be rejected at create"
    );
    assert_eq!(
        h.create_status(json!({ "content": "x", "title": "a".repeat(256) }))
            .await,
        reqwest::StatusCode::BAD_REQUEST,
        "an overlong title must be rejected at create"
    );
    assert_eq!(
        h.create_status(json!({ "content": "x", "description": "a".repeat(4097) }))
            .await,
        reqwest::StatusCode::BAD_REQUEST,
        "an overlong description must be rejected at create"
    );

    let created = h.create(json!({ "content": "v0" })).await;
    let id = created["id"].as_str().expect("id").to_owned();
    assert_eq!(
        h.raw_status(
            reqwest::Method::PATCH,
            &format!("/api/snippets/{id}"),
            Some(&json!({ "content_type": "bad\nvalue" })),
        )
        .await,
        reqwest::StatusCode::BAD_REQUEST,
        "the same content_type validation must apply on update"
    );

    // Exactly at the boundary (255 chars) must be accepted, not rejected.
    let exact_title = "a".repeat(255);
    let boundary = h
        .create(json!({ "content": "boundary", "title": exact_title.clone() }))
        .await;
    assert_eq!(
        boundary["title"],
        json!(exact_title),
        "a title at exactly the length cap must be accepted"
    );
}

// ---------------------------------------------------------------------------
// Content-addressing / ledger idempotence boundaries.
// ---------------------------------------------------------------------------

/// Empty content has no special-cased rejection: it is a legitimate, if
/// degenerate, template and must round-trip through creation and delivery.
#[tokio::test]
async fn empty_content_is_permitted_and_deliverable() {
    let h = Harness::start().await;
    let created = h.create(json!({ "content": "" })).await;
    let id = created["id"].as_str().expect("id").to_owned();
    let (body, _) = h.deliver(&id).await;
    assert_eq!(body, "", "empty content must be stored and served verbatim");
}

/// Two unrelated routes created with byte-identical content dedup to the same
/// immutable content block, but remain fully independent routes: updating one
/// must never affect the other.
#[tokio::test]
async fn identical_content_across_routes_shares_storage_but_stays_independent() {
    let h = Harness::start().await;

    let a = h.create(json!({ "content": "shared payload" })).await;
    let b = h.create(json!({ "content": "shared payload" })).await;
    let a_id = a["id"].as_str().expect("id").to_owned();
    let b_id = b["id"].as_str().expect("id").to_owned();
    assert_ne!(
        a_id, b_id,
        "routes must have distinct random ids even with identical content"
    );

    let a_detail = h.detail(&a_id).await;
    let b_detail = h.detail(&b_id).await;
    let a_hash = a_detail["history"][0]["target_hash"]
        .as_str()
        .expect("a hash");
    let b_hash = b_detail["history"][0]["target_hash"]
        .as_str()
        .expect("b hash");
    assert_eq!(
        a_hash, b_hash,
        "identical content must dedup to the same content-block hash"
    );

    h.update(&a_id, "only A now").await;
    let (body_a, _) = h.deliver(&a_id).await;
    let (body_b, _) = h.deliver(&b_id).await;
    assert_eq!(body_a, "only A now");
    assert_eq!(
        body_b, "shared payload",
        "routes sharing a content block must stay independently mutable"
    );
}

/// Repointing a route at content byte-identical to its current version is
/// still a genuine update: it must append a ledger row like any other update,
/// not be silently treated as a no-op.
#[tokio::test]
async fn updating_to_identical_content_still_appends_history() {
    let h = Harness::start().await;
    let created = h.create(json!({ "content": "same" })).await;
    let id = created["id"].as_str().expect("id").to_owned();

    h.update(&id, "same").await;

    let detail = h.detail(&id).await;
    assert_eq!(
        detail["history_count"].as_u64(),
        Some(2),
        "a content-identical update must still append a ledger row"
    );
}

/// Restoring a route to its own current version is still a genuine restore:
/// it must append a fresh ledger row, matching "every update appends," not be
/// treated as a no-op because the target hash already matches.
#[tokio::test]
async fn restoring_current_version_still_appends_history() {
    let h = Harness::start().await;
    let created = h.create(json!({ "content": "v0" })).await;
    let id = created["id"].as_str().expect("id").to_owned();
    let detail = h.detail(&id).await;
    let current_hash = detail["history"][0]["target_hash"]
        .as_str()
        .expect("current hash")
        .to_owned();

    h.restore(&id, &current_hash).await;

    let detail2 = h.detail(&id).await;
    assert_eq!(
        detail2["history_count"].as_u64(),
        Some(2),
        "restoring the current version must still append a ledger row"
    );
    assert_eq!(detail2["history"][0]["target_hash"], json!(current_hash));
}

// ---------------------------------------------------------------------------
// Pagination boundaries.
// ---------------------------------------------------------------------------

/// An owner with zero routes gets an empty page and no `next_cursor` — the
/// degenerate zero-item boundary, distinct from every other pagination test
/// which starts from at least one item.
#[tokio::test]
async fn empty_snippet_list_has_no_next_cursor() {
    let h = Harness::start().await;
    let page = h.list(&[]).await;
    assert_eq!(page["snippets"].as_array().expect("snippets").len(), 0);
    assert!(page["next_cursor"].is_null());
}

/// Requesting exactly as many rows as exist must not report a `next_cursor` —
/// the classic off-by-one risk in a "fetch limit+1, truncate" pagination
/// scheme, where the boundary is requesting precisely the remaining count.
#[tokio::test]
async fn list_snippets_exact_page_boundary_has_no_next_cursor() {
    let h = Harness::start().await;
    for i in 0..3 {
        h.create(json!({ "content": format!("s{i}") })).await;
    }

    let page = h.list(&[("limit", "3")]).await;
    assert_eq!(page["snippets"].as_array().expect("snippets").len(), 3);
    assert!(
        page["next_cursor"].is_null(),
        "requesting exactly the remaining count must not report a next page"
    );
}

/// The same exact-boundary condition, for the embedded history page on the
/// snippet detail endpoint.
#[tokio::test]
async fn history_exact_page_boundary_has_no_next_cursor() {
    let h = Harness::start().await;
    let created = h.create(json!({ "content": "v0" })).await;
    let id = created["id"].as_str().expect("id").to_owned();
    for i in 1..=4 {
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
    assert_eq!(detail["history"].as_array().expect("history").len(), 5);
    assert!(
        detail["history_next_cursor"].is_null(),
        "requesting exactly the remaining history count must not report a next page"
    );
}

/// A cursor is stateless, MAC-protected pagination *position* — not a
/// server-side consumed token — so replaying the same cursor value twice must
/// yield byte-identical pages.
#[tokio::test]
async fn pagination_cursor_is_idempotent_and_replayable() {
    let h = Harness::start().await;
    for i in 0..5 {
        h.create(json!({ "content": format!("s{i}") })).await;
    }

    let page1 = h.list(&[("limit", "2")]).await;
    let cursor = page1["next_cursor"]
        .as_str()
        .expect("more pages remain")
        .to_owned();

    let page2a = h.list(&[("limit", "2"), ("cursor", &cursor)]).await;
    let page2b = h.list(&[("limit", "2"), ("cursor", &cursor)]).await;
    assert_eq!(
        page2a, page2b,
        "replaying the same cursor must be idempotent"
    );
}

/// A cursor with a valid shape but a tampered MAC (as opposed to plainly
/// malformed base64/JSON) must still be rejected with `400` — proving the MAC
/// is actually checked, not just base64/JSON well-formedness.
#[tokio::test]
async fn tampered_cursor_mac_is_rejected() {
    let h = Harness::start().await;
    for i in 0..3 {
        h.create(json!({ "content": format!("s{i}") })).await;
    }

    let page = h.list(&[("limit", "1")]).await;
    let cursor = page["next_cursor"]
        .as_str()
        .expect("more pages remain")
        .to_owned();
    let mut chars: Vec<char> = cursor.chars().collect();
    let last = chars.len() - 1;
    chars[last] = if chars[last] == 'A' { 'B' } else { 'A' };
    let tampered: String = chars.into_iter().collect();

    assert_eq!(
        h.list_status(&[("cursor", &tampered)]).await,
        reqwest::StatusCode::BAD_REQUEST,
        "a cursor with a tampered MAC must be rejected"
    );
}

/// A history cursor is not tagged with the route it was minted for — only
/// with its endpoint *kind*. Replaying one route's history cursor against a
/// *different* route's history endpoint must never surface rows outside that
/// path route's own ledger: the `route_id` in the URL, not the cursor, is
/// what scopes every returned row.
#[tokio::test]
async fn history_cursor_from_another_route_stays_scoped_to_the_path_route() {
    let h = Harness::start().await;

    let a = h.create(json!({ "content": "a0" })).await;
    let a_id = a["id"].as_str().expect("id").to_owned();
    h.update(&a_id, "a1").await;
    h.update(&a_id, "a2").await;

    let b = h.create(json!({ "content": "b0" })).await;
    let b_id = b["id"].as_str().expect("id").to_owned();
    h.update(&b_id, "b1").await;
    h.update(&b_id, "b2").await;

    let a_page = h.history_page(&a_id, &[("limit", "1")]).await;
    let a_cursor = a_page["next_cursor"]
        .as_str()
        .expect("route A has more history")
        .to_owned();

    let b_full = h.history_page(&b_id, &[("limit", "50")]).await;
    let b_hashes: Vec<String> = b_full["history"]
        .as_array()
        .expect("history")
        .iter()
        .map(|e| e["target_hash"].as_str().expect("hash").to_owned())
        .collect();

    let resp = h
        .client
        .get(format!("{}/api/snippets/{b_id}/history", h.control_base))
        .query(&[("cursor", a_cursor.as_str())])
        .send()
        .await
        .expect("cross-route history request");
    assert!(
        resp.status().is_success() || resp.status() == reqwest::StatusCode::BAD_REQUEST,
        "a cross-route cursor must fail cleanly, not 500"
    );
    if resp.status().is_success() {
        let page: Value = resp.json().await.expect("history json");
        for item in page["history"].as_array().expect("history") {
            let hash = item["target_hash"].as_str().expect("hash").to_owned();
            assert!(
                b_hashes.contains(&hash),
                "a history cursor minted for another route must never surface rows outside \
                 the path route's own ledger"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Delivery hardening and rendering edge cases.
// ---------------------------------------------------------------------------

/// Every delivery response carries the hardened, content-agnostic security
/// headers regardless of the stored content type — proving the defusing
/// happens at the response-header layer, not by rewriting attacker content.
#[tokio::test]
async fn delivery_responses_carry_hardened_security_headers() {
    let h = Harness::start().await;
    let created = h
        .create(json!({
            "content": "<script>alert(1)</script>",
            "content_type": "text/html; charset=utf-8"
        }))
        .await;
    let id = created["id"].as_str().expect("id").to_owned();

    let (_, headers) = h.deliver(&id).await;
    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    assert_eq!(
        headers.get("referrer-policy").and_then(|v| v.to_str().ok()),
        Some("no-referrer")
    );
    assert_eq!(
        headers
            .get("content-security-policy")
            .and_then(|v| v.to_str().ok()),
        Some("default-src 'none'; sandbox")
    );
}

/// A repeated query key is not a rendering hazard: `form_urlencoded`'s
/// last-value-wins semantics apply, deterministically, end-to-end.
#[tokio::test]
async fn repeated_query_params_use_the_last_value() {
    let h = Harness::start().await;
    let created = h.create(json!({ "content": "{{name}}" })).await;
    let id = created["id"].as_str().expect("id").to_owned();

    let (body, _) = h.deliver(&format!("{id}?name=first&name=second")).await;
    assert_eq!(
        body, "second",
        "a duplicated query key must resolve deterministically to the last value"
    );
}
