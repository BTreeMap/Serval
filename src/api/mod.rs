//! The Control Plane: management API plus the embedded dashboard.

mod error;
mod extract;
mod handlers;
mod pagination;
mod static_files;

use axum::Router;
use axum::routing::{get, post};
use tower_http::cors::CorsLayer;

pub use error::ApiError;

use crate::state::ControlState;

/// Build the Control Plane router: the JSON management API under `/api`, with
/// the embedded SPA served for everything else.
pub fn router(state: ControlState) -> Router {
    // The dashboard authenticates with Bearer tokens (not cookies), so a
    // permissive CORS policy is safe and lets a separate Vite dev server call
    // the API during development.
    let cors = CorsLayer::permissive();

    Router::new()
        .route("/api/auth-info", get(handlers::auth_info))
        .route("/api/me", get(handlers::me))
        .route(
            "/api/snippets",
            get(handlers::list_snippets).post(handlers::create_snippet),
        )
        .route(
            "/api/snippets/{id}",
            get(handlers::get_snippet).patch(handlers::update_snippet),
        )
        .route(
            "/api/snippets/{id}/versions/{hash}",
            get(handlers::get_version),
        )
        .route(
            "/api/snippets/{id}/history",
            get(handlers::get_snippet_history),
        )
        .route(
            "/api/snippets/{id}/restore",
            post(handlers::restore_snippet),
        )
        .fallback(static_files::serve)
        .layer(cors)
        .with_state(state)
}
