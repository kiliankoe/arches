//! The React UI, embedded in the binary.
//!
//! `web/dist` is gitignored and a fresh checkout has none, so `build.rs` creates it empty and
//! this falls back to a note instead of a 404 the user cannot act on.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "web/dist"]
struct Assets;

const NOT_BUILT: &str = "The arches UI was not built into this binary. \
Run `pnpm --dir web build` and rebuild, or use the API under /api.\n";

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if let Some(response) = file(path) {
        return response;
    }
    // Anything that is not a file is a client-side route, so the SPA shell answers it.
    match file("index.html") {
        Some(response) => response,
        None => (StatusCode::OK, NOT_BUILT).into_response(),
    }
}

fn file(path: &str) -> Option<Response> {
    let asset = Assets::get(path)?;
    let content_type = asset.metadata.mimetype().to_string();
    // The shell names the hashed bundles, so it must never be the stale part of a deploy.
    let cache_control = if path == "index.html" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    Some(
        (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, cache_control.to_string()),
            ],
            asset.data,
        )
            .into_response(),
    )
}
