//! Serves the single-page web UI embedded in the binary.

use axum::{
    http::{HeaderMap, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "public/"]
struct Assets;

pub async fn static_handler(uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');
    let (path, file) = match Assets::get(path).filter(|_| !path.is_empty()) {
        Some(f) => (path, f),
        // Unknown paths fall back to the app shell so client-side routes work.
        None if !path.contains('.') => ("index.html", Assets::get("index.html").expect("index.html is embedded")),
        None => return (StatusCode::NOT_FOUND, "Not found").into_response(),
    };
    let etag = format!("\"{}\"", file.metadata.sha256_hash().iter().take(12).map(|b| format!("{b:02x}")).collect::<String>());
    if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    (
        [
            (header::CONTENT_TYPE, mime.as_ref().to_string()),
            (header::ETAG, etag),
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        file.data,
    )
        .into_response()
}
