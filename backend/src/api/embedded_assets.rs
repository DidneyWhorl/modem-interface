//! Embedded frontend asset serving via rust-embed.
//!
//! When the `embedded-frontend` feature is enabled, the frontend dist/
//! directory is compiled into the binary. No files on disk needed.

#[cfg(feature = "embedded-frontend")]
mod inner {
    use axum::{
        body::Body,
        http::{header, HeaderValue, Request, StatusCode},
        response::{IntoResponse, Response},
    };
    use rust_embed::Embed;

    #[derive(Embed)]
    #[folder = "../frontend/dist"]
    struct FrontendAssets;

    /// Serve embedded frontend assets. Falls back to index.html for SPA routing.
    ///
    /// The handler is nested under `/ctrl-modem`, so Axum strips that prefix
    /// before we see the path. We just need to trim the leading slash.
    pub async fn serve_embedded(req: Request<Body>) -> Response {
        let path = req.uri().path().trim_start_matches('/');

        // Try the exact path first
        if let Some(response) = serve_file(path) {
            return response;
        }

        // SPA fallback: serve index.html for any unmatched path
        if let Some(response) = serve_file("index.html") {
            return response;
        }

        StatusCode::NOT_FOUND.into_response()
    }

    fn serve_file(path: &str) -> Option<Response> {
        let file = FrontendAssets::get(path)?;
        let mime = mime_guess::from_path(path).first_or_octet_stream();

        let mut response = Response::builder()
            .header(header::CONTENT_TYPE, mime.as_ref());

        // Long cache for hashed assets, no-cache for index.html
        if path.contains("/assets/") || path.starts_with("assets/") {
            response = response.header(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
        } else {
            response = response.header(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-cache"),
            );
        }

        Some(
            response
                .body(Body::from(file.data.to_vec()))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        )
    }
}

#[cfg(feature = "embedded-frontend")]
pub use inner::serve_embedded;
