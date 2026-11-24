//! The articulation area: one page and its assets, in the binary.
//!
//! The page has no build step: plain HTML, CSS and JavaScript. It calls the API on the
//! same origin (`/api/v1/diogenes/...`), so the host session cookie works with no change.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;

const INDEX: &str = include_str!("index.html");
const APP_JS: &str = include_str!("app.js");
const APP_CSS: &str = include_str!("app.css");

/// The Content-Security-Policy of the page: same-origin only, plus Câmara photos.
const CSP: &str = "default-src 'self'; img-src 'self' https://www.camara.leg.br data:; \
                   style-src 'self'; script-src 'self'; connect-src 'self'; \
                   frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

pub(crate) fn routes() -> Router<()> {
    Router::new()
        .route(
            "/articulacao",
            get(|| async { Redirect::permanent("/articulacao/") }),
        )
        .route("/articulacao/", get(index))
        .route(
            "/articulacao/app.js",
            get(|| async { asset(APP_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/articulacao/app.css",
            get(|| async { asset(APP_CSS, "text/css; charset=utf-8") }),
        )
}

async fn index() -> Response {
    let mut response = (StatusCode::OK, INDEX).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    response
}

fn asset(body: &'static str, content_type: &'static str) -> Response {
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
