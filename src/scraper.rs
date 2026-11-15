use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, bail};
use chrono::Utc;

use crate::{
    AppState,
    cogo::FetchError,
    db::{self, RecordStats},
    util,
};

/// Voorkomt dat twee scrapes tegelijk lopen.
#[derive(Default)]
pub struct ScrapeLock(AtomicBool);

impl ScrapeLock {
    pub fn is_running(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

struct Guard<'a>(&'a ScrapeLock);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        (self.0).0.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RunSummary {
    pub run_id: i64,
    pub products: u64,
    pub total: Option<u64>,
    pub stats: RecordStats,
    pub urls_mapped: u64,
}

/// Voert één volledige scrape uit. Geeft `Ok(None)` als er al een scrape bezig is.
pub async fn run_once(state: &AppState) -> Result<Option<RunSummary>> {
    if state.scrape_lock.0.swap(true, Ordering::SeqCst) {
        return Ok(None);
    }
    let _guard = Guard(&state.scrape_lock);

    let started = Utc::now();
    let run_id = db::start_run(&state.db, started).await?;
    tracing::info!(run_id, "scrape gestart");

    let result = scrape_products(state, run_id, started).await;
    let (products, total, stats, mut error) = match result {
        Ok((p, t, s)) => (p, t, s, None),
        Err((p, t, s, e)) => (p, t, s, Some(format!("{e:#}"))),
    };

    // Ook bij een gedeeltelijke scrape de URL-koppelingen bijwerken.
    let urls_mapped = if products > 0 {
        match sync_sitemap(state).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "sitemap-synchronisatie mislukt");
                let msg = format!("sitemap: {e:#}");
                error = Some(match error {
                    Some(prev) => format!("{prev}; {msg}"),
                    None => msg,
                });
                0
            }
        }
    } else {
        0
    };

    let complete = total.is_none_or(|t| products * 100 >= t * 98);
    let status = match (products, complete) {
        (0, _) => "failed",
        (_, true) if error.is_none() => "success",
        _ => "partial",
    };
    db::finish_run(&state.db, run_id, status, urls_mapped, error.clone()).await?;
    tracing::info!(
        run_id,
        status,
        products,
        total,
        new = stats.new_products,
        changed = stats.prices_changed,
        urls_mapped,
        error,
        elapsed_s = (Utc::now() - started).num_seconds(),
        "scrape klaar"
    );
    if status == "failed" {
        bail!("scrape mislukt: {}", error.unwrap_or_default());
    }
    Ok(Some(RunSummary {
        run_id,
        products,
        total,
        stats,
        urls_mapped,
    }))
}

type ScrapeResult =
    Result<(u64, Option<u64>, RecordStats), (u64, Option<u64>, RecordStats, anyhow::Error)>;

async fn scrape_products(
    state: &AppState,
    run_id: i64,
    seen_at: chrono::DateTime<Utc>,
) -> ScrapeResult {
    let page_size = state.config.page_size;
    let mut seen: HashSet<String> = HashSet::new();
    let mut stats = RecordStats::default();
    let mut total: Option<u64> = None;
    let mut page = 1u32;
    let mut empty_pages = 0;

    loop {
        let resp = match state.cogo.search_page(page, page_size).await {
            Ok(r) => r,
            Err(e) => {
                return Err((
                    seen.len() as u64,
                    total,
                    stats,
                    e.context(format!("pagina {page}")),
                ));
            }
        };
        if let Some(t) = resp.total {
            total = Some(t);
        }
        let batch: Vec<_> = resp
            .entries
            .iter()
            .filter(|e| seen.insert(e.unique_id.clone()))
            .map(|e| e.normalize())
            .collect();
        if resp.entries.is_empty() {
            empty_pages += 1;
        }
        match db::record_products(&state.db, &batch, seen_at).await {
            Ok(s) => {
                stats.new_products += s.new_products;
                stats.prices_changed += s.prices_changed;
            }
            Err(e) => {
                return Err((
                    seen.len() as u64,
                    total,
                    stats,
                    e.context("opslaan mislukt"),
                ));
            }
        }
        let _ = db::update_run_progress(&state.db, run_id, seen.len() as u64, total, stats).await;
        tracing::info!(
            page,
            got = resp.entries.len(),
            seen = seen.len(),
            ?total,
            "pagina verwerkt"
        );

        let pages_needed = total.map(|t| t.div_ceil(u64::from(page_size))).unwrap_or(0);
        let max_pages = state.config.max_pages;
        if empty_pages > 0
            || u64::from(page) >= pages_needed
            || page >= 1000
            || (max_pages > 0 && page >= max_pages)
        {
            break;
        }
        page += 1;
        tokio::time::sleep(state.cogo.request_delay()).await;
    }
    Ok((seen.len() as u64, total, stats))
}

/// Leest de product-sitemaps en koppelt URL's aan producten via hun afbeelding.
async fn sync_sitemap(state: &AppState) -> Result<u64> {
    let sitemaps = state.cogo.sitemap_product_urls().await?;
    let by_asset = db::products_by_image_asset(&state.db).await?;
    let mut mapped = 0;
    for sm in sitemaps {
        tokio::time::sleep(state.cogo.request_delay()).await;
        let entries = match state.cogo.sitemap(&sm).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(sitemap = %sm, error = %e, "sitemap ophalen mislukt");
                continue;
            }
        };
        let mut links = Vec::new();
        for entry in entries {
            let Some(cands) = entry.image_asset.as_ref().and_then(|a| by_asset.get(a)) else {
                continue;
            };
            let product_id = if cands.len() == 1 {
                Some(&cands[0].0)
            } else {
                // Meerdere producten met dezelfde afbeelding: vergelijk op naam.
                let title = entry.title.as_deref().map(util::search_normalize);
                let matches: Vec<_> = cands
                    .iter()
                    .filter(|(_, name)| Some(util::search_normalize(name)) == title)
                    .collect();
                (matches.len() == 1).then(|| &matches[0].0)
            };
            let Some(product_id) = product_id else {
                continue;
            };
            for u in &entry.urls {
                if let Ok(p) = util::parse_product_url(u) {
                    links.push((p.key, p.lang, product_id.clone()));
                }
            }
        }
        mapped += db::upsert_urls(&state.db, &links, "sitemap").await?;
    }
    Ok(mapped)
}

/// Draait voor altijd: scrapet elke `scrape_interval`.
pub async fn scheduler(state: Arc<AppState>) {
    let interval = state.config.scrape_interval;
    let mut first = true;
    loop {
        let wait = match db::last_successful_run(&state.db).await {
            Ok(Some(run)) => {
                let since = (Utc::now() - run.started_at).to_std().unwrap_or_default();
                interval.saturating_sub(since)
            }
            Ok(None) => Duration::ZERO,
            Err(e) => {
                tracing::error!(error = %e, "kan laatste run niet lezen");
                Duration::from_secs(60)
            }
        };
        let wait = if first && !state.config.scrape_on_startup && wait.is_zero() {
            interval
        } else {
            wait
        };
        first = false;
        if !wait.is_zero() {
            tracing::info!(next_in_min = wait.as_secs() / 60, "volgende scrape gepland");
            tokio::time::sleep(wait).await;
        }
        match run_once(&state).await {
            Ok(Some(_)) => {}
            Ok(None) => tracing::info!("scrape al bezig, overgeslagen"),
            Err(e) => {
                tracing::error!(error = %format!("{e:#}"), "scrape mislukt, opnieuw over 30 minuten");
                tokio::time::sleep(Duration::from_secs(30 * 60).min(interval)).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// URL -> product
// ---------------------------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error(transparent)]
    BadUrl(#[from] util::UrlError),
    #[error("geen product gevonden voor deze URL")]
    NotFound,
    #[error("Collect&Go is momenteel niet bereikbaar: {0}")]
    Upstream(String),
    #[error("interne fout: {0}")]
    Internal(#[from] anyhow::Error),
}

const NEGATIVE_CACHE: chrono::Duration = chrono::Duration::hours(1);

/// Zoekt het product-ID voor een Collect&Go-link: een productpagina of een zoeklink met
/// `#pdp_<id>`.
pub async fn resolve_url(state: &AppState, raw: &str) -> Result<String, ResolveError> {
    match util::parse_cogo_link(raw)? {
        util::CogoLink::Page(path) => resolve_page(state, &path).await,
        util::CogoLink::Sku(sku) => resolve_sku(state, &sku).await,
    }
}

/// Een product-ID voor een SKU-ID uit `#pdp_<id>`.
///
/// Collect&Go (HCL Commerce) maakt elk product samen met zijn ene SKU aan, met opeenvolgende
/// ID's: het product heeft het ID van de SKU min één. Is dat product gekend, dan volstaat de
/// database. Anders vragen we het product op en aanvaarden het enkel als de SKU erbij hoort.
async fn resolve_sku(state: &AppState, sku: &str) -> Result<String, ResolveError> {
    if db::product_exists(&state.db, sku).await? {
        return Ok(sku.to_string());
    }
    let parent = sku
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .ok_or(ResolveError::NotFound)?
        .to_string();
    if db::product_exists(&state.db, &parent).await? {
        return Ok(parent);
    }

    let key = format!("#pdp_{sku}");
    if db::recently_unresolved(&state.db, &key, Utc::now() - NEGATIVE_CACHE)
        .await?
        .is_some()
    {
        return Err(ResolveError::NotFound);
    }
    let entry = state
        .cogo
        .product_by_id(&parent)
        .await
        .map_err(|e| ResolveError::Upstream(format!("{e:#}")))?;
    let Some(entry) =
        entry.filter(|e| e.unique_id == parent && e.skus.iter().any(|s| s.unique_id == sku))
    else {
        db::mark_unresolved(&state.db, &key, "SKU niet gevonden").await?;
        return Err(ResolveError::NotFound);
    };
    db::record_products(&state.db, &[entry.normalize()], Utc::now()).await?;
    Ok(parent)
}

/// Product-ID voor een productpagina. Eerst in de database (sitemap-koppelingen), anders wordt
/// de productpagina één keer opgehaald en het resultaat bewaard.
async fn resolve_page(state: &AppState, path: &util::ProductPath) -> Result<String, ResolveError> {
    if let Some(id) = db::product_for_path(&state.db, &path.key).await? {
        return Ok(id);
    }
    if db::recently_unresolved(&state.db, &path.key, Utc::now() - NEGATIVE_CACHE)
        .await?
        .is_some()
    {
        return Err(ResolveError::NotFound);
    }

    let page = match state.cogo.product_page(&path.request_path).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            db::mark_unresolved(&state.db, &path.key, "geen productpagina").await?;
            return Err(ResolveError::NotFound);
        }
        Err(FetchError::NotFound) => {
            db::mark_unresolved(&state.db, &path.key, "404").await?;
            return Err(ResolveError::NotFound);
        }
        Err(FetchError::Other(e)) => return Err(ResolveError::Upstream(format!("{e:#}"))),
    };

    // Product nog niet gekend (bv. nieuw sinds de laatste scrape): rechtstreeks ophalen.
    if !db::product_exists(&state.db, &page.product_id).await? {
        let entry = state
            .cogo
            .product_by_id(&page.product_id)
            .await
            .map_err(|e| ResolveError::Upstream(format!("{e:#}")))?;
        let Some(entry) = entry else {
            db::mark_unresolved(&state.db, &path.key, "product onbekend in API").await?;
            return Err(ResolveError::NotFound);
        };
        db::record_products(&state.db, &[entry.normalize()], Utc::now()).await?;
    }

    let mut links = vec![(path.key.clone(), path.lang, page.product_id.clone())];
    for u in &page.urls {
        if let Ok(p) = util::parse_product_url(u)
            && p.key != path.key
        {
            links.push((p.key, p.lang, page.product_id.clone()));
        }
    }
    db::upsert_urls(&state.db, &links, "page").await?;
    Ok(page.product_id)
}
