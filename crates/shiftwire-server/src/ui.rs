//! Serves the web UI embedded from `ui/dist` (single-page app).

use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::Embed)]
#[folder = "../../ui/dist"]
struct Assets;

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Unknown paths get index.html so client-side routes survive a reload.
    let (path, file) = match Assets::get(path).filter(|_| !path.is_empty()) {
        Some(file) => (path, file),
        None => match Assets::get("index.html") {
            Some(file) => ("index.html", file),
            None => return StatusCode::NOT_FOUND.into_response(),
        },
    };
    // Vite fingerprints everything under assets/, so it can be cached forever.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let headers = [
        (CONTENT_TYPE, file.metadata.mimetype().to_owned()),
        (CACHE_CONTROL, cache.to_owned()),
    ];
    (headers, file.data).into_response()
}
