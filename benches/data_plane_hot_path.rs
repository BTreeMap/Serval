use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use axum::http::HeaderValue;
use bytes::Bytes;
use divan::black_box;
use serval::cache::CachedSnippet;
use serval::crypto::{ETAG_HEADER_LEN, IdSigner};
use serval::db::models::{CacheMode, RouteId};
use serval::renderer;

const SECRET: &str = "benchmark-secret-with-at-least-thirty-two-bytes";

static SIGNER: LazyLock<IdSigner> = LazyLock::new(|| IdSigner::new(SECRET));
static VALID_ID: LazyLock<String> = LazyLock::new(|| SIGNER.random_id());
static CONTENT_ID: LazyLock<String> = LazyLock::new(|| SIGNER.content_id(STATIC_CONTENT));
static ETAG_BYTES: LazyLock<[u8; ETAG_HEADER_LEN]> =
    LazyLock::new(|| SIGNER.etag_bytes(&CONTENT_ID, SHORT_QUERY.as_bytes()));
static ETAG_STRING: LazyLock<String> =
    LazyLock::new(|| SIGNER.etag(&CONTENT_ID, SHORT_QUERY.as_bytes()));
static CONTENT_TYPE_HEADER: LazyLock<HeaderValue> =
    LazyLock::new(|| HeaderValue::from_static("text/plain; charset=utf-8"));
static ARC_LONG_CONTENT: LazyLock<Arc<str>> =
    LazyLock::new(|| Arc::from(LONG_STATIC_CONTENT.as_str()));
static ARC_TARGET_HASH: LazyLock<Arc<str>> = LazyLock::new(|| Arc::from(CONTENT_ID.as_str()));
static CACHED_SNIPPET: LazyLock<Arc<CachedSnippet>> = LazyLock::new(|| {
    Arc::new(CachedSnippet {
        content: LONG_STATIC_CONTENT.clone().into_boxed_str(),
        content_type: CONTENT_TYPE_HEADER.clone(),
        cache_mode: CacheMode::Mutable,
        target_hash: CONTENT_ID.clone().into_boxed_str(),
    })
});
static OWNED_SNIPPET: LazyLock<Arc<OwnedSnippet>> = LazyLock::new(|| {
    Arc::new(OwnedSnippet {
        content: LONG_STATIC_CONTENT.clone(),
    })
});
static FORGED_ID: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
static STATIC_CONTENT: &str =
    "Serval serves this placeholder-free snippet directly from the data plane cache.\n";
static TEMPLATE_CONTENT: &str = "listen={{port}} tenant={{tenant}} missing={{uuid}}\n";
static LONG_STATIC_CONTENT: LazyLock<String> = LazyLock::new(|| {
    let mut content = String::with_capacity(32 * 1024);
    for line in 0..512 {
        content.push_str("static config line ");
        content.push_str(&line.to_string());
        content.push_str(" = value with enough bytes to exercise regex scanning\n");
    }
    content
});
static MANY_PLACEHOLDERS: LazyLock<String> = LazyLock::new(|| {
    let mut content = String::with_capacity(16 * 1024);
    for index in 0..512 {
        content.push_str("key");
        content.push_str(&index.to_string());
        content.push_str("={{port}} tenant={{tenant}} missing={{uuid}}\n");
    }
    content
});
static VARIABLES: LazyLock<HashMap<Cow<'static, str>, Cow<'static, str>>> = LazyLock::new(|| {
    HashMap::from([
        (Cow::Borrowed("port"), Cow::Borrowed("3000")),
        (Cow::Borrowed("tenant"), Cow::Borrowed("prod")),
    ])
});
static EMPTY_VARIABLES: LazyLock<HashMap<Cow<'static, str>, Cow<'static, str>>> =
    LazyLock::new(HashMap::new);
static SHORT_QUERY: &str = "port=3000&tenant=prod";
static LONG_QUERY: LazyLock<String> = LazyLock::new(|| {
    let mut query = String::with_capacity(16 * 1024);
    query.push_str("port=3000&tenant=prod");
    for index in 0..512 {
        query.push('&');
        query.push_str("noise");
        query.push_str(&index.to_string());
        query.push('=');
        query.push_str("value");
        query.push_str(&index.to_string());
    }
    query
});

struct ArcStrOwner(Arc<str>);

impl AsRef<[u8]> for ArcStrOwner {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

struct OwnedSnippet {
    content: String,
}

struct ArcSnippetOwner(Arc<OwnedSnippet>);

impl AsRef<[u8]> for ArcSnippetOwner {
    fn as_ref(&self) -> &[u8] {
        self.0.content.as_bytes()
    }
}

fn main() {
    divan::main();
}

#[divan::bench]
fn route_id_parse_valid() {
    black_box(RouteId::parse(black_box(&VALID_ID)).unwrap());
}

#[divan::bench]
fn route_id_parse_wrong_length() {
    black_box(RouteId::parse(black_box("too-short")).is_err());
}

#[divan::bench]
fn signer_verify_valid_route_id() {
    black_box(SIGNER.verify(black_box(&VALID_ID)));
}

#[divan::bench]
fn signer_verify_valid_content_id() {
    black_box(SIGNER.verify(black_box(&CONTENT_ID)));
}

#[divan::bench]
fn signer_verify_forged_well_formed_id() {
    black_box(SIGNER.verify(black_box(FORGED_ID)));
}

#[divan::bench]
fn signer_mutable_etag_short_query() {
    black_box(SIGNER.etag(black_box(&CONTENT_ID), black_box(SHORT_QUERY.as_bytes())));
}

#[divan::bench]
fn signer_mutable_etag_bytes_short_query() {
    black_box(SIGNER.etag_bytes(black_box(&CONTENT_ID), black_box(SHORT_QUERY.as_bytes())));
}

#[divan::bench]
fn signer_mutable_etag_long_query() {
    black_box(SIGNER.etag(black_box(&CONTENT_ID), black_box(LONG_QUERY.as_bytes())));
}

#[divan::bench]
fn signer_mutable_etag_bytes_long_query() {
    black_box(SIGNER.etag_bytes(black_box(&CONTENT_ID), black_box(LONG_QUERY.as_bytes())));
}

#[divan::bench]
fn header_value_from_etag_bytes() {
    black_box(HeaderValue::from_bytes(black_box(&*ETAG_BYTES)).unwrap());
}

#[divan::bench]
fn header_value_from_etag_str() {
    black_box(HeaderValue::from_str(black_box(&ETAG_STRING)).unwrap());
}

#[divan::bench]
fn header_value_from_owned_bytes_checked() {
    let bytes = Bytes::from_owner(black_box(*ETAG_BYTES));
    black_box(HeaderValue::from_maybe_shared(black_box(bytes)).unwrap());
}

#[divan::bench]
fn header_value_from_owned_bytes_unchecked() {
    let bytes = Bytes::from_owner(black_box(*ETAG_BYTES));
    black_box(unsafe { HeaderValue::from_maybe_shared_unchecked(black_box(bytes)) });
}

#[divan::bench]
fn mutable_etag_header_from_string() {
    let etag = SIGNER.etag(black_box(&CONTENT_ID), black_box(SHORT_QUERY.as_bytes()));
    black_box(HeaderValue::from_str(black_box(&etag)).unwrap());
}

#[divan::bench]
fn mutable_etag_header_from_bytes() {
    let etag = SIGNER.etag_bytes(black_box(&CONTENT_ID), black_box(SHORT_QUERY.as_bytes()));
    black_box(HeaderValue::from_bytes(black_box(&etag)).unwrap());
}

#[divan::bench]
fn mutable_etag_header_from_owned_bytes_checked() {
    let etag = SIGNER.etag_bytes(black_box(&CONTENT_ID), black_box(SHORT_QUERY.as_bytes()));
    let bytes = Bytes::from_owner(etag);
    black_box(HeaderValue::from_maybe_shared(black_box(bytes)).unwrap());
}

#[divan::bench]
fn content_type_header_from_str() {
    black_box(HeaderValue::from_str(black_box("text/plain; charset=utf-8")).unwrap());
}

#[divan::bench]
fn content_type_header_clone() {
    black_box(CONTENT_TYPE_HEADER.clone());
}

#[divan::bench]
fn arc_cached_snippet_clone() {
    black_box(Arc::clone(black_box(&*CACHED_SNIPPET)));
}

#[divan::bench]
fn arc_content_clone_long_content() {
    black_box(Arc::clone(black_box(&*ARC_LONG_CONTENT)));
}

#[divan::bench]
fn string_clone_long_content() {
    black_box(black_box(&*LONG_STATIC_CONTENT).clone());
}

#[divan::bench]
fn arc_target_hash_clone() {
    black_box(Arc::clone(black_box(&*ARC_TARGET_HASH)));
}

#[divan::bench]
fn arc_str_from_target_hash() {
    black_box(Arc::<str>::from(black_box(CONTENT_ID.as_str())));
}

#[divan::bench]
fn string_clone_target_hash() {
    black_box(black_box(&*CONTENT_ID).clone());
}

#[divan::bench]
fn bytes_from_owner_arc_str_long_content() {
    let owner = ArcStrOwner(Arc::clone(black_box(&*ARC_LONG_CONTENT)));
    black_box(Bytes::from_owner(owner));
}

#[divan::bench]
fn bytes_from_owner_whole_snippet_long_content() {
    let owner = ArcSnippetOwner(Arc::clone(black_box(&*OWNED_SNIPPET)));
    black_box(Bytes::from_owner(owner));
}

#[divan::bench]
fn bytes_copy_from_slice_long_content() {
    black_box(Bytes::copy_from_slice(black_box(
        LONG_STATIC_CONTENT.as_bytes(),
    )));
}

#[divan::bench]
fn bytes_from_owned_string_long_content() {
    let content = black_box(&*LONG_STATIC_CONTENT).clone();
    black_box(Bytes::from(content));
}

#[divan::bench]
fn parse_short_query() {
    black_box(parse_query(black_box(SHORT_QUERY)));
}

#[divan::bench]
fn parse_short_query_owned_strings() {
    black_box(parse_query_owned(black_box(SHORT_QUERY)));
}

#[divan::bench]
fn parse_long_query() {
    black_box(parse_query(black_box(&LONG_QUERY)));
}

#[divan::bench]
fn parse_long_query_owned_strings() {
    black_box(parse_query_owned(black_box(&LONG_QUERY)));
}

#[divan::bench]
fn render_static_short_content_empty_vars() {
    black_box(renderer::render(
        black_box(STATIC_CONTENT),
        black_box(&EMPTY_VARIABLES),
    ));
}

#[divan::bench]
fn render_static_long_content_empty_vars() {
    black_box(renderer::render(
        black_box(&LONG_STATIC_CONTENT),
        black_box(&EMPTY_VARIABLES),
    ));
}

#[divan::bench]
fn render_template_with_substitutions() {
    black_box(renderer::render(
        black_box(TEMPLATE_CONTENT),
        black_box(&VARIABLES),
    ));
}

#[divan::bench]
fn render_many_placeholders_with_substitutions() {
    black_box(renderer::render(
        black_box(&MANY_PLACEHOLDERS),
        black_box(&VARIABLES),
    ));
}

#[divan::bench]
fn render_query_static_short_content_short_query() {
    black_box(renderer::render_query(
        black_box(STATIC_CONTENT),
        black_box(SHORT_QUERY),
    ));
}

#[divan::bench]
fn render_query_static_long_content_long_query() {
    black_box(renderer::render_query(
        black_box(&LONG_STATIC_CONTENT),
        black_box(&LONG_QUERY),
    ));
}

#[divan::bench]
fn render_query_template_short_query() {
    black_box(renderer::render_query(
        black_box(TEMPLATE_CONTENT),
        black_box(SHORT_QUERY),
    ));
}

#[divan::bench]
fn render_query_many_placeholders_long_query() {
    black_box(renderer::render_query(
        black_box(&MANY_PLACEHOLDERS),
        black_box(&LONG_QUERY),
    ));
}

fn parse_query(query: &str) -> HashMap<Cow<'_, str>, Cow<'_, str>> {
    form_urlencoded::parse(query.as_bytes()).collect()
}

fn parse_query_owned(query: &str) -> HashMap<String, String> {
    form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}
