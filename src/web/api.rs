use axum::{
    Json,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    State,
    view::{HistoryPointJson, ProductPriceJson, image_url},
};
use crate::{
    bol::{self, BolError, BolProductPriceJson},
    db,
    scraper::{self, ResolveError},
    util::cents_to_f64,
};

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({ "error": self.1, "status": self.0.as_u16() })),
        )
            .into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!(error = %format!("{e:#}"), "API-fout");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "interne fout".into())
    }
}

impl From<ResolveError> for ApiError {
    fn from(e: ResolveError) -> Self {
        let status = match &e {
            ResolveError::BadUrl(_) => StatusCode::BAD_REQUEST,
            ResolveError::NotFound => StatusCode::NOT_FOUND,
            ResolveError::Upstream(_) => StatusCode::BAD_GATEWAY,
            ResolveError::Internal(err) => {
                tracing::error!(error = %format!("{err:#}"), "fout bij URL-resolutie");
                return ApiError(StatusCode::INTERNAL_SERVER_ERROR, "interne fout".into());
            }
        };
        ApiError(status, e.to_string())
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

pub async fn not_found() -> ApiError {
    ApiError(
        StatusCode::NOT_FOUND,
        "onbekend endpoint, zie /api voor de documentatie".into(),
    )
}

#[derive(Deserialize)]
pub struct PriceQuery {
    url: Option<String>,
}

impl From<BolError> for ApiError {
    fn from(e: BolError) -> Self {
        let status = match &e {
            BolError::BadUrl(_) => StatusCode::BAD_REQUEST,
            BolError::NotFound => StatusCode::NOT_FOUND,
            BolError::Upstream(_) => StatusCode::BAD_GATEWAY,
        };
        ApiError(status, e.to_string())
    }
}

/// `GET /api/v1/price?url=<collect&go- of bol.com-productlink>`
pub async fn price_by_url(state: State, Query(q): Query<PriceQuery>) -> Result<Response, ApiError> {
    let url = q
        .url
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "parameter `url` ontbreekt".into()))?;
    if bol::is_bol_url(&url) {
        let (product, seen_at) = state.bol.product(&url).await?;
        return Ok(Json(BolProductPriceJson::new(product, seen_at)).into_response());
    }
    Ok(cogo_price(&state, &url).await?.into_response())
}

async fn cogo_price(state: &crate::AppState, url: &str) -> ApiResult<ProductPriceJson> {
    let id = scraper::resolve_url(state, url).await?;
    let row = db::product(&state.db, &id)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "product niet gevonden".into()))?;
    Ok(Json(ProductPriceJson::from_row(
        &row,
        &state.config.public_base_url,
    )))
}

/// `GET /api/v1/products/{id}` — id = Collect&Go product-ID of artikelnummer.
pub async fn product(state: State, Path(id): Path<String>) -> ApiResult<ProductPriceJson> {
    let row = match db::find_product_id(&state.db, &id).await? {
        Some(id) => db::product(&state.db, &id).await?,
        None => None,
    }
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "product niet gevonden".into()))?;
    Ok(Json(ProductPriceJson::from_row(
        &row,
        &state.config.public_base_url,
    )))
}

#[derive(Serialize)]
pub struct HistoryJson {
    product_id: String,
    name: String,
    currency: &'static str,
    history: Vec<HistoryPointJson>,
}

/// `GET /api/v1/products/{id}/history`
pub async fn history(state: State, Path(id): Path<String>) -> ApiResult<HistoryJson> {
    let row = match db::find_product_id(&state.db, &id).await? {
        Some(id) => db::product(&state.db, &id).await?,
        None => None,
    }
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "product niet gevonden".into()))?;
    let history = db::price_history(&state.db, &row.product.id).await?;
    Ok(Json(HistoryJson {
        product_id: row.product.id.clone(),
        name: row.product.name.clone(),
        currency: "EUR",
        history: history.iter().map(HistoryPointJson::from).collect(),
    }))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
    sort: Option<String>,
    available: Option<bool>,
    page: Option<i64>,
    per_page: Option<i64>,
}

#[derive(Serialize)]
pub struct SearchJson {
    query: String,
    total: i64,
    page: i64,
    per_page: i64,
    results: Vec<ProductPriceJson>,
}

/// `GET /api/v1/products?q=...&page=1&per_page=24&sort=prijs&available=true`
pub async fn search(state: State, Query(q): Query<SearchQuery>) -> ApiResult<SearchJson> {
    let query = q.q.unwrap_or_default();
    let per_page = q.per_page.unwrap_or(24).clamp(1, 100);
    let page = q.page.unwrap_or(1).max(1);
    let (rows, total) = db::search(
        &state.db,
        &query,
        db::Sort::parse(q.sort.as_deref()),
        q.available.unwrap_or(false),
        per_page,
        (page - 1) * per_page,
    )
    .await?;
    Ok(Json(SearchJson {
        query,
        total,
        page,
        per_page,
        results: rows
            .iter()
            .map(|r| ProductPriceJson::from_row(r, &state.config.public_base_url))
            .collect(),
    }))
}

#[derive(Deserialize)]
pub struct ChangesQuery {
    days: Option<i64>,
    direction: Option<String>,
    limit: Option<i64>,
}

#[derive(Serialize)]
pub struct ChangeJson {
    product_id: String,
    name: String,
    brand: Option<String>,
    content: Option<String>,
    image_url: Option<String>,
    old_price: f64,
    new_price: f64,
    change_percent: f64,
    changed_at: chrono::DateTime<Utc>,
}

/// `GET /api/v1/changes?days=7&direction=down|up|all&limit=50`
pub async fn changes(state: State, Query(q): Query<ChangesQuery>) -> ApiResult<Vec<ChangeJson>> {
    let days = q.days.unwrap_or(7).clamp(1, 365);
    let dir = match q.direction.as_deref() {
        Some("down") => -1,
        Some("up") => 1,
        _ => 0,
    };
    let rows = db::recent_changes(
        &state.db,
        Utc::now() - chrono::Duration::days(days),
        dir,
        q.limit.unwrap_or(50).clamp(1, 500),
    )
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|c| ChangeJson {
                change_percent: ((c.new_price - c.old_price) as f64 / c.old_price as f64 * 1000.0)
                    .round()
                    / 10.0,
                old_price: cents_to_f64(c.old_price),
                new_price: cents_to_f64(c.new_price),
                image_url: image_url(c.image_path.as_deref()),
                product_id: c.product_id,
                name: c.name,
                brand: c.brand,
                content: c.content,
                changed_at: c.changed_at,
            })
            .collect(),
    ))
}

#[derive(Serialize)]
pub struct StatusJson {
    stats: db::Stats,
    scrape_running: bool,
    scrape_interval_hours: f64,
    transport: crate::cogo::TransportStatus,
    last_successful_run: Option<db::ScrapeRun>,
    recent_runs: Vec<db::ScrapeRun>,
}

/// `GET /api/v1/status`
pub async fn status(state: State) -> ApiResult<StatusJson> {
    Ok(Json(StatusJson {
        stats: db::stats(&state.db).await?,
        scrape_running: state.scrape_lock.is_running(),
        scrape_interval_hours: state.config.scrape_interval.as_secs_f64() / 3600.0,
        transport: state.cogo.transport_status(),
        last_successful_run: db::last_successful_run(&state.db).await?,
        recent_runs: db::recent_runs(&state.db, 10).await?,
    }))
}

/// `POST /api/v1/admin/scrape` met `Authorization: Bearer <ADMIN_TOKEN>`.
pub async fn trigger_scrape(
    state: State,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let Some(token) = state.config.admin_token.as_deref() else {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "admin-endpoint is uitgeschakeld (geen ADMIN_TOKEN)".into(),
        ));
    };
    let given = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !constant_time_eq(given.as_bytes(), token.as_bytes()) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "ongeldig token".into()));
    }
    if state.scrape_lock.is_running() {
        return Ok((
            StatusCode::CONFLICT,
            Json(json!({ "status": "already_running" })),
        ));
    }
    let st = state.0.clone();
    tokio::spawn(async move {
        if let Err(e) = scraper::run_once(&st).await {
            tracing::error!(error = %format!("{e:#}"), "handmatige scrape mislukt");
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "started" }))))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
