use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use axum::http::HeaderValue;
use bytes::Bytes;
use divan::black_box;
use serval::crypto::{ETAG_HEADER_LEN, IdSigner};
use serval::db::models::RouteId;
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
fn parse_short_query() {
    black_box(parse_query(black_box(SHORT_QUERY)));
}

#[divan::bench]
fn parse_long_query() {
    black_box(parse_query(black_box(&LONG_QUERY)));
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
