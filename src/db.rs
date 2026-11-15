use std::{collections::HashMap, str::FromStr, time::Duration};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{
    // Dynamische SQL bestaat enkel uit constanten; waarden gaan altijd via bind().
    AssertSqlSafe,
    FromRow,
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::{
    cogo::{PriceObs, ScrapedProduct},
    util,
};

pub type Db = SqlitePool;

pub async fn connect(url: &str) -> Result<Db> {
    if let Some(path) = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
    {
        let path = path.split('?').next().unwrap_or(path);
        if let Some(parent) = std::path::Path::new(path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("kan map {} niet aanmaken", parent.display()))?;
        }
    }
    let opts = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(30))
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("migraties uitvoeren mislukt")?;
    Ok(pool)
}

// ---------------------------------------------------------------------------------------------
// Rijen
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ProductRow {
    pub id: String,
    pub part_number: String,
    pub tech_article_id: Option<String>,
    pub name: String,
    pub long_name: Option<String>,
    pub brand: Option<String>,
    pub content: Option<String>,
    pub unit: Option<String>,
    pub image_path: Option<String>,
    pub offered_by: Option<String>,
    pub is_available: bool,
    pub is_ranged: bool,
    pub promotions: String,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct PriceRow {
    pub id: i64,
    pub product_id: String,
    pub base_price: i64,
    pub unit_price: Option<i64>,
    pub offer_price: Option<i64>,
    pub offer_quantity: Option<i64>,
    pub offer_unit_price: Option<i64>,
    pub reference_price: Option<i64>,
    pub is_red_price: bool,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

impl PriceRow {
    fn obs(&self) -> PriceObs {
        PriceObs {
            base_price: self.base_price,
            unit_price: self.unit_price,
            offer_price: self.offer_price,
            offer_quantity: self.offer_quantity,
            offer_unit_price: self.offer_unit_price,
            reference_price: self.reference_price,
            is_red_price: self.is_red_price,
        }
    }
}

/// Product + huidige prijs + vorige prijs, voor lijsten en detailpagina's.
#[derive(Debug, Clone, FromRow)]
pub struct ProductWithPrice {
    #[sqlx(flatten)]
    pub product: ProductRow,
    pub base_price: Option<i64>,
    pub unit_price: Option<i64>,
    pub offer_price: Option<i64>,
    pub offer_quantity: Option<i64>,
    pub offer_unit_price: Option<i64>,
    pub reference_price: Option<i64>,
    pub is_red_price: Option<bool>,
    pub price_first_seen_at: Option<DateTime<Utc>>,
    pub price_last_seen_at: Option<DateTime<Utc>>,
    pub previous_price: Option<i64>,
    pub url_path: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ScrapeRun {
    pub id: i64,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: String,
    pub products_seen: i64,
    pub products_total: Option<i64>,
    pub new_products: i64,
    pub prices_changed: i64,
    pub urls_mapped: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct PriceChange {
    pub product_id: String,
    pub name: String,
    pub brand: Option<String>,
    pub content: Option<String>,
    pub image_path: Option<String>,
    pub old_price: i64,
    pub new_price: i64,
    pub changed_at: DateTime<Utc>,
}

const PRODUCT_COLS: &str = "p.id, p.part_number, p.tech_article_id, p.name, p.long_name, p.brand, p.content, p.unit, \
     p.image_path, p.offered_by, p.is_available, p.is_ranged, p.promotions, p.first_seen_at, p.last_seen_at";

fn product_with_price_select() -> String {
    format!(
        "SELECT {PRODUCT_COLS},
            c.base_price, c.unit_price, c.offer_price, c.offer_quantity, c.offer_unit_price,
            c.reference_price, c.is_red_price, c.first_seen_at AS price_first_seen_at, c.last_seen_at AS price_last_seen_at,
            (SELECT pp.base_price FROM prices pp WHERE pp.product_id = p.id
               AND (pp.first_seen_at < c.first_seen_at OR (pp.first_seen_at = c.first_seen_at AND pp.id < c.id))
               ORDER BY pp.first_seen_at DESC, pp.id DESC LIMIT 1) AS previous_price,
            (SELECT u.path FROM product_urls u WHERE u.product_id = p.id
               ORDER BY (u.lang = 'nl') DESC, (u.source = 'sitemap') DESC LIMIT 1) AS url_path
         FROM products p LEFT JOIN prices c ON c.id = p.current_price_id"
    )
}

// ---------------------------------------------------------------------------------------------
// Schrijven
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy)]
pub struct RecordStats {
    pub new_products: u64,
    pub prices_changed: u64,
}

/// Slaat een batch gescrapete producten op in één transactie.
/// Een nieuwe prijsrij wordt enkel aangemaakt als de prijs verschilt van de huidige.
pub async fn record_products(
    db: &Db,
    products: &[ScrapedProduct],
    seen_at: DateTime<Utc>,
) -> Result<RecordStats> {
    let mut stats = RecordStats::default();
    let mut tx = db.begin().await?;
    for p in products {
        let promotions = serde_json::to_string(&p.promotions)?;
        let search_text = util::search_normalize(&format!(
            "{} {} {} {} {}",
            p.name,
            p.long_name.as_deref().unwrap_or(""),
            p.brand.as_deref().unwrap_or(""),
            p.content.as_deref().unwrap_or(""),
            p.part_number.trim_end_matches("-P"),
        ));
        let inserted = sqlx::query(
            "INSERT INTO products (id, part_number, tech_article_id, name, long_name, brand, content, unit, image_path,
                image_asset, offered_by, is_available, is_ranged, promotions, search_text, first_seen_at, last_seen_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                part_number = excluded.part_number, tech_article_id = excluded.tech_article_id, name = excluded.name,
                long_name = excluded.long_name, brand = excluded.brand, content = excluded.content, unit = excluded.unit,
                image_path = excluded.image_path, image_asset = excluded.image_asset, offered_by = excluded.offered_by,
                is_available = excluded.is_available, is_ranged = excluded.is_ranged, promotions = excluded.promotions,
                search_text = excluded.search_text, last_seen_at = excluded.last_seen_at
             RETURNING (first_seen_at = ?) AS is_new",
        )
        .bind(&p.id)
        .bind(&p.part_number)
        .bind(&p.tech_article_id)
        .bind(&p.name)
        .bind(&p.long_name)
        .bind(&p.brand)
        .bind(&p.content)
        .bind(&p.unit)
        .bind(&p.image_path)
        .bind(&p.image_asset)
        .bind(&p.offered_by)
        .bind(p.is_available)
        .bind(p.is_ranged)
        .bind(&promotions)
        .bind(&search_text)
        .bind(seen_at)
        .bind(seen_at)
        .bind(seen_at)
        .fetch_one(&mut *tx)
        .await?;
        let is_new: bool = sqlx::Row::try_get(&inserted, "is_new").unwrap_or(false);
        if is_new {
            stats.new_products += 1;
        }

        let Some(obs) = &p.price else { continue };
        let current: Option<PriceRow> = sqlx::query_as(
            "SELECT c.* FROM prices c JOIN products p ON p.current_price_id = c.id WHERE p.id = ?",
        )
        .bind(&p.id)
        .fetch_optional(&mut *tx)
        .await?;

        match current {
            Some(cur) if &cur.obs() == obs => {
                if seen_at > cur.last_seen_at {
                    sqlx::query("UPDATE prices SET last_seen_at = ? WHERE id = ?")
                        .bind(seen_at)
                        .bind(cur.id)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            Some(cur) if seen_at <= cur.last_seen_at => {
                // Oudere waarneming dan wat we al hebben: negeren.
            }
            current => {
                if current.is_some() {
                    stats.prices_changed += 1;
                }
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO prices (product_id, base_price, unit_price, offer_price, offer_quantity, offer_unit_price,
                        reference_price, is_red_price, first_seen_at, last_seen_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
                )
                .bind(&p.id)
                .bind(obs.base_price)
                .bind(obs.unit_price)
                .bind(obs.offer_price)
                .bind(obs.offer_quantity)
                .bind(obs.offer_unit_price)
                .bind(obs.reference_price)
                .bind(obs.is_red_price)
                .bind(seen_at)
                .bind(seen_at)
                .fetch_one(&mut *tx)
                .await?;
                sqlx::query("UPDATE products SET current_price_id = ? WHERE id = ?")
                    .bind(id)
                    .bind(&p.id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(stats)
}

/// Koppelt URL-paden aan een product. `source = "page"` (rechtstreeks van de productpagina
/// gelezen) overschrijft altijd; `"sitemap"` overschrijft nooit een `"page"`-koppeling.
pub async fn upsert_urls(
    db: &Db,
    entries: &[(String, &'static str, String)],
    source: &str,
) -> Result<u64> {
    let now = Utc::now();
    let mut n = 0;
    let mut tx = db.begin().await?;
    for (path, lang, product_id) in entries {
        let res = sqlx::query(
            "INSERT INTO product_urls (path, product_id, lang, source, updated_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(path) DO UPDATE SET product_id = excluded.product_id, source = excluded.source,
                updated_at = excluded.updated_at
             WHERE excluded.source = 'page' OR product_urls.source = 'sitemap'",
        )
        .bind(path)
        .bind(product_id)
        .bind(*lang)
        .bind(source)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        n += res.rows_affected();
        sqlx::query("DELETE FROM unresolved_urls WHERE path = ?")
            .bind(path)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(n)
}

/// image_asset -> [(product_id, name)]
pub async fn products_by_image_asset(db: &Db) -> Result<HashMap<String, Vec<(String, String)>>> {
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT image_asset, id, name FROM products WHERE image_asset IS NOT NULL")
            .fetch_all(db)
            .await?;
    let mut map: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (asset, id, name) in rows {
        map.entry(asset).or_default().push((id, name));
    }
    Ok(map)
}

pub async fn product_for_path(db: &Db, path: &str) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT u.product_id FROM product_urls u JOIN products p ON p.id = u.product_id WHERE u.path = ?",
    )
    .bind(path)
    .fetch_optional(db)
    .await?)
}

pub async fn recently_unresolved(
    db: &Db,
    path: &str,
    since: DateTime<Utc>,
) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT reason FROM unresolved_urls WHERE path = ? AND checked_at > ?")
            .bind(path)
            .bind(since)
            .fetch_optional(db)
            .await?,
    )
}

pub async fn mark_unresolved(db: &Db, path: &str, reason: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO unresolved_urls (path, reason, checked_at) VALUES (?, ?, ?)
         ON CONFLICT(path) DO UPDATE SET reason = excluded.reason, checked_at = excluded.checked_at",
    )
    .bind(path)
    .bind(reason)
    .bind(Utc::now())
    .execute(db)
    .await?;
    Ok(())
}

pub async fn product_exists(db: &Db, id: &str) -> Result<bool> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM products WHERE id = ?")
            .bind(id)
            .fetch_one(db)
            .await?
            > 0,
    )
}

// Scrape-runs -----------------------------------------------------------------------------------

pub async fn start_run(db: &Db, started_at: DateTime<Utc>) -> Result<i64> {
    // Runs die nog op 'running' staan zijn onderbroken (bv. door een herstart).
    sqlx::query("UPDATE scrape_runs SET status = 'failed', error = 'onderbroken', finished_at = ? WHERE status = 'running'")
        .bind(started_at)
        .execute(db)
        .await?;
    Ok(sqlx::query_scalar(
        "INSERT INTO scrape_runs (started_at, status) VALUES (?, 'running') RETURNING id",
    )
    .bind(started_at)
    .fetch_one(db)
    .await?)
}

pub async fn update_run_progress(
    db: &Db,
    id: i64,
    seen: u64,
    total: Option<u64>,
    stats: RecordStats,
) -> Result<()> {
    sqlx::query(
        "UPDATE scrape_runs SET products_seen = ?, products_total = ?, new_products = ?, prices_changed = ? WHERE id = ?",
    )
    .bind(seen as i64)
    .bind(total.map(|t| t as i64))
    .bind(stats.new_products as i64)
    .bind(stats.prices_changed as i64)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn finish_run(
    db: &Db,
    id: i64,
    status: &str,
    urls_mapped: u64,
    error: Option<String>,
) -> Result<()> {
    sqlx::query("UPDATE scrape_runs SET status = ?, finished_at = ?, urls_mapped = ?, error = ? WHERE id = ?")
        .bind(status)
        .bind(Utc::now())
        .bind(urls_mapped as i64)
        .bind(error)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn last_successful_run(db: &Db) -> Result<Option<ScrapeRun>> {
    Ok(sqlx::query_as(
        "SELECT * FROM scrape_runs WHERE status IN ('success', 'partial') ORDER BY started_at DESC LIMIT 1",
    )
    .fetch_optional(db)
    .await?)
}

pub async fn recent_runs(db: &Db, limit: i64) -> Result<Vec<ScrapeRun>> {
    Ok(
        sqlx::query_as("SELECT * FROM scrape_runs ORDER BY id DESC LIMIT ?")
            .bind(limit)
            .fetch_all(db)
            .await?,
    )
}

// ---------------------------------------------------------------------------------------------
// Lezen
// ---------------------------------------------------------------------------------------------

pub async fn product(db: &Db, id: &str) -> Result<Option<ProductWithPrice>> {
    Ok(sqlx::query_as(AssertSqlSafe(format!(
        "{} WHERE p.id = ?",
        product_with_price_select()
    )))
    .bind(id)
    .fetch_optional(db)
    .await?)
}

/// Product op basis van ID of artikelnummer (bv. "28856" of "28856-P").
pub async fn find_product_id(db: &Db, key: &str) -> Result<Option<String>> {
    let key = key.trim();
    Ok(sqlx::query_scalar(
        "SELECT id FROM products WHERE id = ?1 OR part_number = ?1 OR part_number = ?1 || '-P' OR tech_article_id = ?1
         ORDER BY (id = ?1) DESC LIMIT 1",
    )
    .bind(key)
    .fetch_optional(db)
    .await?)
}

pub async fn price_history(db: &Db, product_id: &str) -> Result<Vec<PriceRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM prices WHERE product_id = ? ORDER BY first_seen_at ASC, id ASC",
    )
    .bind(product_id)
    .fetch_all(db)
    .await?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Relevance,
    PriceAsc,
    PriceDesc,
    Name,
    RecentlyChanged,
}

impl Sort {
    pub fn parse(s: Option<&str>) -> Self {
        match s.unwrap_or("") {
            "prijs" | "price" | "price_asc" => Sort::PriceAsc,
            "prijs_desc" | "price_desc" => Sort::PriceDesc,
            "naam" | "name" => Sort::Name,
            "gewijzigd" | "changed" => Sort::RecentlyChanged,
            _ => Sort::Relevance,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Sort::Relevance => "relevantie",
            Sort::PriceAsc => "prijs",
            Sort::PriceDesc => "prijs_desc",
            Sort::Name => "naam",
            Sort::RecentlyChanged => "gewijzigd",
        }
    }
}

/// Zoekt producten. Elk woord in de zoekterm moet voorkomen (AND).
pub async fn search(
    db: &Db,
    query: &str,
    sort: Sort,
    only_available: bool,
    limit: i64,
    offset: i64,
) -> Result<(Vec<ProductWithPrice>, i64)> {
    let norm = util::search_normalize(query);
    let terms: Vec<String> = norm
        .split(' ')
        .filter(|t| !t.is_empty())
        .take(8)
        .map(String::from)
        .collect();
    let mut where_sql = String::from(" WHERE 1=1");
    for _ in &terms {
        where_sql.push_str(" AND (' ' || p.search_text) LIKE ?");
    }
    if only_available {
        where_sql.push_str(" AND p.is_available = 1");
    }
    let order = match sort {
        Sort::Relevance if !terms.is_empty() => {
            "ORDER BY (' ' || p.search_text) LIKE ? DESC, p.is_available DESC, length(p.name) ASC, p.name ASC"
        }
        Sort::Relevance | Sort::Name => "ORDER BY p.name COLLATE NOCASE ASC",
        Sort::PriceAsc => "ORDER BY c.base_price IS NULL, c.base_price ASC, p.name ASC",
        Sort::PriceDesc => "ORDER BY c.base_price IS NULL, c.base_price DESC, p.name ASC",
        Sort::RecentlyChanged => "ORDER BY c.first_seen_at DESC, p.name ASC",
    };

    let count_sql = format!("SELECT COUNT(*) FROM products p{where_sql}");
    let mut count_q = sqlx::query_scalar::<_, i64>(AssertSqlSafe(count_sql));
    for t in &terms {
        count_q = count_q.bind(format!("% {t}%"));
    }
    let total = count_q.fetch_one(db).await?;

    let sql = format!(
        "{}{where_sql} {order} LIMIT ? OFFSET ?",
        product_with_price_select()
    );
    let mut q = sqlx::query_as::<_, ProductWithPrice>(AssertSqlSafe(sql));
    for t in &terms {
        q = q.bind(format!("% {t}%"));
    }
    if sort == Sort::Relevance && !terms.is_empty() {
        // Producten waarvan de volledige zoekterm als woordreeks voorkomt eerst.
        q = q.bind(format!("% {norm}%"));
    }
    let rows = q.bind(limit).bind(offset).fetch_all(db).await?;
    Ok((rows, total))
}

/// Recente prijswijzigingen (enkel echte wijzigingen, geen eerste waarnemingen).
pub async fn recent_changes(
    db: &Db,
    since: DateTime<Utc>,
    direction: i32,
    limit: i64,
) -> Result<Vec<PriceChange>> {
    let order = if direction < 0 {
        "(CAST(new_price AS REAL) - old_price) / old_price ASC"
    } else if direction > 0 {
        "(CAST(new_price AS REAL) - old_price) / old_price DESC"
    } else {
        "changed_at DESC"
    };
    let filter = if direction < 0 {
        "AND new_price < old_price"
    } else if direction > 0 {
        "AND new_price > old_price"
    } else {
        ""
    };
    let sql = format!(
        "WITH ch AS (
            SELECT c.product_id, c.base_price AS new_price, c.first_seen_at AS changed_at,
                   (SELECT pp.base_price FROM prices pp WHERE pp.product_id = c.product_id
                     AND (pp.first_seen_at < c.first_seen_at OR (pp.first_seen_at = c.first_seen_at AND pp.id < c.id))
                     ORDER BY pp.first_seen_at DESC, pp.id DESC LIMIT 1) AS old_price
            FROM products p JOIN prices c ON c.id = p.current_price_id
            WHERE c.first_seen_at >= ?
        )
        SELECT ch.product_id, p.name, p.brand, p.content, p.image_path, ch.old_price, ch.new_price, ch.changed_at
        FROM ch JOIN products p ON p.id = ch.product_id
        WHERE ch.old_price IS NOT NULL AND ch.old_price > 0 AND ch.old_price <> ch.new_price {filter}
        ORDER BY {order} LIMIT ?"
    );
    Ok(sqlx::query_as(AssertSqlSafe(sql))
        .bind(since)
        .bind(limit)
        .fetch_all(db)
        .await?)
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub products: i64,
    pub available_products: i64,
    pub price_points: i64,
    pub mapped_urls: i64,
}

pub async fn stats(db: &Db) -> Result<Stats> {
    let (products, available_products): (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(is_available), 0) FROM products")
            .fetch_one(db)
            .await?;
    let price_points: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM prices")
        .fetch_one(db)
        .await?;
    let mapped_urls: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM product_urls")
        .fetch_one(db)
        .await?;
    Ok(Stats {
        products,
        available_products,
        price_points,
        mapped_urls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as CDuration;

    fn sp(id: &str, price: i64) -> ScrapedProduct {
        ScrapedProduct {
            id: id.into(),
            part_number: format!("{id}-P"),
            tech_article_id: Some(id.into()),
            name: format!("Product {id}"),
            long_name: None,
            brand: Some("Boni".into()),
            content: Some("200g".into()),
            unit: Some("kg".into()),
            image_path: Some("JPG/200x200/asset-1.jpg".into()),
            image_asset: Some(format!("a{id}")),
            offered_by: None,
            is_available: true,
            is_ranged: true,
            promotions: vec![],
            price: Some(PriceObs {
                base_price: price,
                unit_price: Some(price * 5),
                offer_price: None,
                offer_quantity: None,
                offer_unit_price: None,
                reference_price: None,
                is_red_price: false,
            }),
        }
    }

    async fn mem_db() -> Db {
        connect("sqlite::memory:").await.unwrap()
    }

    #[tokio::test]
    async fn records_only_changes() {
        let db = mem_db().await;
        let t0 = Utc::now();
        let t1 = t0 + CDuration::hours(6);
        let t2 = t0 + CDuration::hours(12);

        let s = record_products(&db, &[sp("1", 319), sp("2", 100)], t0)
            .await
            .unwrap();
        assert_eq!((s.new_products, s.prices_changed), (2, 0));

        let s = record_products(&db, &[sp("1", 319), sp("2", 120)], t1)
            .await
            .unwrap();
        assert_eq!((s.new_products, s.prices_changed), (0, 1));

        let s = record_products(&db, &[sp("1", 319), sp("2", 120)], t2)
            .await
            .unwrap();
        assert_eq!((s.new_products, s.prices_changed), (0, 0));

        let h1 = price_history(&db, "1").await.unwrap();
        assert_eq!(h1.len(), 1);
        assert_eq!(h1[0].first_seen_at, t0);
        assert_eq!(h1[0].last_seen_at, t2);

        let h2 = price_history(&db, "2").await.unwrap();
        assert_eq!(h2.len(), 2);
        assert_eq!(h2[1].base_price, 120);
        assert_eq!(h2[1].first_seen_at, t1);
        assert_eq!(h2[1].last_seen_at, t2);

        let p2 = product_row(&db, "2").await;
        assert_eq!(p2.base_price, Some(120));
        assert_eq!(p2.previous_price, Some(100));
        assert_eq!(p2.price_first_seen_at, Some(t1));
        assert_eq!(p2.price_last_seen_at, Some(t2));

        let changes = recent_changes(&db, t0 - CDuration::days(1), 0, 10)
            .await
            .unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!((changes[0].old_price, changes[0].new_price), (100, 120));

        // Woorden matchen op prefix: "2" matcht ook "200g" van product 1, maar de exacte
        // woordreeks "product 2" moet bovenaan staan.
        let (rows, total) = search(&db, "product 2", Sort::Relevance, false, 10, 0)
            .await
            .unwrap();
        assert_eq!(total, 2);
        assert_eq!(rows[0].product.id, "2");
        let (_, total) = search(&db, "nietsbestaands", Sort::Relevance, false, 10, 0)
            .await
            .unwrap();
        assert_eq!(total, 0);
        let (_, total) = search(&db, "boni", Sort::PriceAsc, true, 10, 0)
            .await
            .unwrap();
        assert_eq!(total, 2);
    }

    async fn product_row(db: &Db, id: &str) -> ProductWithPrice {
        super::product(db, id).await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn url_mapping_precedence() {
        let db = mem_db().await;
        record_products(&db, &[sp("1", 100), sp("2", 200)], Utc::now())
            .await
            .unwrap();
        upsert_urls(
            &db,
            &[("/nl/assortiment/x".into(), "nl", "1".into())],
            "page",
        )
        .await
        .unwrap();
        // sitemap mag een page-koppeling niet overschrijven
        upsert_urls(
            &db,
            &[("/nl/assortiment/x".into(), "nl", "2".into())],
            "sitemap",
        )
        .await
        .unwrap();
        assert_eq!(
            product_for_path(&db, "/nl/assortiment/x")
                .await
                .unwrap()
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            find_product_id(&db, "2").await.unwrap().as_deref(),
            Some("2")
        );
        assert_eq!(
            find_product_id(&db, "1-P").await.unwrap().as_deref(),
            Some("1")
        );
    }
}
