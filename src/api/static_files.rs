//! Serve the embedded React dashboard.
//!
//! The compiled Vite output in `frontend/dist/` is embedded into the binary at
//! build time via `rust-embed`. Unknown non-asset paths fall back to
//! `index.html` so the client-side router can handle them (SPA behavior).

use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct Assets;

/// Fallback handler: serve the requested asset, or `index.html` for SPA routes.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let candidate = if path.is_empty() { "index.html" } else { path };

    match Assets::get(candidate) {
        Some(file) => asset_response(candidate, file.data.into_owned()),
        // Unknown path: hand control to the SPA router via index.html.
        None => match Assets::get("index.html") {
            Some(index) => asset_response("index.html", index.data.into_owned()),
            None => (
                StatusCode::NOT_FOUND,
                "frontend assets are not embedded in this build",
            )
                .into_response(),
        },
    }
}

fn asset_response(path: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    (
        [(header::CONTENT_TYPE, mime.as_ref().to_owned())],
        Body::from(bytes),
    )
        .into_response()
}
