use axum::{
    http::{header, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use rust_embed::Embed;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

use catalogy_catalog::Catalog;
use catalogy_search::SearchEngine;

use crate::api;

#[derive(Embed)]
#[folder = "src/static_files/"]
struct StaticAssets;

pub struct AppState {
    pub catalog: Arc<Catalog>,
    pub search_engine: Option<SearchEngine>,
}

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/search", post(api::search_handler))
        .route("/api/media/{id}", get(api::media_handler))
        .route("/api/thumb/{id}", get(api::thumb_handler))
        .route("/api/stats", get(api::stats_handler))
        .route("/", get(index_handler))
        .route("/{*path}", get(static_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn index_handler() -> impl IntoResponse {
    match StaticAssets::get("index.html") {
        Some(content) => Html(
            std::str::from_utf8(content.data.as_ref())
                .unwrap_or("")
                .to_string(),
        )
        .into_response(),
        None => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
    }
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    match StaticAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string();
            Response::builder()
                .header(header::CONTENT_TYPE, mime)
                .body(axum::body::Body::from(content.data.to_vec()))
                .unwrap()
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}
