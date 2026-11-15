mod api;
mod chart;
mod pages;
pub mod view;

use std::sync::Arc;

use axum::{
    Router,
    http::{HeaderValue, header},
    response::IntoResponse,
    routing::{get, post},
};
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use crate::AppState;

pub type State = axum::extract::State<Arc<AppState>>;

pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/price", get(api::price_by_url))
        .route("/products", get(api::search))
        .route("/products/{id}", get(api::product))
        .route("/products/{id}/history", get(api::history))
        .route("/changes", get(api::changes))
        .route("/status", get(api::status))
        .route("/admin/scrape", post(api::trigger_scrape))
        .fallback(api::not_found)
        .layer(CorsLayer::permissive());

    Router::new()
        .route("/", get(pages::index))
        .route("/zoek", get(pages::search))
        .route("/opzoeken", get(pages::lookup))
        .route("/product/{id}", get(pages::product))
        .route("/wijzigingen", get(pages::changes))
        .route("/api", get(pages::api_docs))
        .route("/privacy", get(pages::privacy))
        .route("/static/style.css", get(style_css))
        .route("/static/favicon.svg", get(favicon))
        .route("/static/chart.js", get(chart_js))
        .route(
            "/static/archivo-latin.woff2",
            get(|| font(include_bytes!("../../static/archivo-latin.woff2"))),
        )
        .route(
            "/static/archivo-latin-ext.woff2",
            get(|| font(include_bytes!("../../static/archivo-latin-ext.woff2"))),
        )
        .route("/healthz", get(|| async { "ok" }))
        .nest("/api/v1", api)
        .fallback(pages::not_found)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn style_css() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/css; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
        ],
        include_str!("../../static/style.css"),
    )
}

async fn favicon() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("image/svg+xml"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400"),
            ),
        ],
        include_str!("../../static/favicon.svg"),
    )
}

async fn chart_js() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/javascript; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
        ],
        include_str!("../../static/chart.js"),
    )
}

async fn font(bytes: &'static [u8]) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("font/woff2")),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            ),
        ],
        bytes,
    )
}
