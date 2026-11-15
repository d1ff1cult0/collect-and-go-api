use askama::Template;
use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use chrono::{DateTime, Utc};
use chrono_tz::Europe::Brussels;
use serde::Deserialize;

use super::{
    State, chart,
    view::{image_url, promotions, site_url},
};
use crate::{
    db::{self, PriceChange, ProductWithPrice, Sort},
    scraper::{self, ResolveError},
    util,
};

// ---------------------------------------------------------------------------------------------
// Opmaak-helpers
// ---------------------------------------------------------------------------------------------

const MONTHS: [&str; 12] = [
    "jan", "feb", "mrt", "apr", "mei", "jun", "jul", "aug", "sep", "okt", "nov", "dec",
];

pub fn fmt_euro(cents: i64) -> String {
    util::fmt_cents(cents)
}

pub fn fmt_datetime(t: DateTime<Utc>) -> String {
    let l = t.with_timezone(&Brussels);
    use chrono::{Datelike, Timelike};
    format!(
        "{} {} {}, {:02}:{:02}",
        l.day(),
        MONTHS[l.month0() as usize],
        l.year(),
        l.hour(),
        l.minute()
    )
}

pub fn fmt_date(t: DateTime<Utc>) -> String {
    let l = t.with_timezone(&Brussels);
    use chrono::Datelike;
    format!("{} {} {}", l.day(), MONTHS[l.month0() as usize], l.year())
}

pub fn fmt_date_short(t: DateTime<Utc>) -> String {
    let l = t.with_timezone(&Brussels);
    use chrono::Datelike;
    format!("{} {}", l.day(), MONTHS[l.month0() as usize])
}

/// "3 uur geleden", "gisteren", ...
pub fn fmt_ago(t: DateTime<Utc>) -> String {
    let d = Utc::now() - t;
    let mins = d.num_minutes();
    if mins < 1 {
        "zonet".into()
    } else if mins < 60 {
        format!("{mins} min geleden")
    } else if d.num_hours() < 24 {
        let h = d.num_hours();
        format!("{h} uur geleden")
    } else if d.num_days() == 1 {
        "gisteren".into()
    } else if d.num_days() < 60 {
        format!("{} dagen geleden", d.num_days())
    } else {
        fmt_date(t)
    }
}

fn fmt_int(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// Een prijs opgesplitst als op een schaplabel: euro's groot, centen klein.
pub struct Tag {
    pub euros: String,
    pub cents: String,
}

impl Tag {
    fn new(c: i64) -> Self {
        Self {
            euros: (c / 100).to_string(),
            cents: format!("{:02}", c % 100),
        }
    }
}

pub struct Delta {
    /// "down" (goedkoper), "up" (duurder)
    pub dir: &'static str,
    pub symbol: &'static str,
    pub label: &'static str,
    pub amount: String,
    pub percent: String,
}

impl Delta {
    fn new(old: i64, new: i64) -> Option<Self> {
        if old == new || old <= 0 {
            return None;
        }
        let pct = (new - old) as f64 / old as f64 * 100.0;
        let (dir, symbol, label) = if new < old {
            ("down", "▼", "goedkoper")
        } else {
            ("up", "▲", "duurder")
        };
        Some(Self {
            dir,
            symbol,
            label,
            amount: format!("€ {}", fmt_euro((new - old).abs())),
            percent: format!("{:.1}%", pct.abs()).replace('.', ","),
        })
    }
}

pub struct ProductLine {
    pub id: String,
    pub name: String,
    pub brand: String,
    pub content: String,
    pub image: Option<String>,
    pub price: Option<String>,
    pub unit_price: Option<String>,
    pub delta: Option<Delta>,
    pub available: bool,
    pub has_promo: bool,
    pub is_red: bool,
}

impl ProductLine {
    fn from_row(r: &ProductWithPrice) -> Self {
        let p = &r.product;
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            brand: p.brand.clone().unwrap_or_default(),
            content: p.content.clone().unwrap_or_default(),
            image: image_url(p.image_path.as_deref()),
            price: r.base_price.map(fmt_euro),
            unit_price: unit_price(r.unit_price, p.unit.as_deref()),
            delta: match (r.previous_price, r.base_price) {
                (Some(o), Some(n)) => Delta::new(o, n),
                _ => None,
            },
            available: p.is_available,
            has_promo: p.promotions.len() > 2,
            is_red: r.is_red_price.unwrap_or(false),
        }
    }
}

fn unit_price(cents: Option<i64>, unit: Option<&str>) -> Option<String> {
    let c = cents?;
    Some(match unit.filter(|u| !u.is_empty()) {
        Some(u) => format!("€ {} / {}", fmt_euro(c), u.to_lowercase()),
        None => format!("€ {} per eenheid", fmt_euro(c)),
    })
}

pub struct ChangeLine {
    pub id: String,
    pub name: String,
    pub brand: String,
    pub content: String,
    pub image: Option<String>,
    pub old_price: String,
    pub new_price: String,
    pub delta: Option<Delta>,
    pub when: String,
}

impl From<PriceChange> for ChangeLine {
    fn from(c: PriceChange) -> Self {
        Self {
            delta: Delta::new(c.old_price, c.new_price),
            id: c.product_id,
            name: c.name,
            brand: c.brand.unwrap_or_default(),
            content: c.content.unwrap_or_default(),
            image: image_url(c.image_path.as_deref()),
            old_price: fmt_euro(c.old_price),
            new_price: fmt_euro(c.new_price),
            when: fmt_date_short(c.changed_at),
        }
    }
}

/// Gedeelde gegevens voor de layout.
pub struct Chrome {
    pub title: String,
    pub description: String,
    pub q: String,
    pub nav: &'static str,
    pub last_update: Option<String>,
}

impl Chrome {
    async fn new(state: &State, title: impl Into<String>, nav: &'static str) -> Self {
        let last_update = db::last_successful_run(&state.db)
            .await
            .ok()
            .flatten()
            .map(|r| fmt_ago(r.finished_at.unwrap_or(r.started_at)));
        Self {
            title: title.into(),
            description:
                "Volg de prijzen van alle producten op Collect&Go. Elke zes uur bijgewerkt.".into(),
            q: String::new(),
            nav,
            last_update,
        }
    }
}

fn render<T: Template>(t: &T) -> Response {
    match t.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "template-fout");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Er ging iets mis bij het tonen van deze pagina.",
            )
                .into_response()
        }
    }
}

pub struct PageError(StatusCode, String);

impl From<anyhow::Error> for PageError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!(error = %format!("{e:#}"), "paginafout");
        PageError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Er ging iets mis aan onze kant. Probeer het zo meteen opnieuw.".into(),
        )
    }
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        let chrome = Chrome {
            title: if self.0 == StatusCode::NOT_FOUND {
                "Niet gevonden".into()
            } else {
                "Fout".into()
            },
            description: String::new(),
            q: String::new(),
            nav: "",
            last_update: None,
        };
        let mut r = render(&ErrorPage {
            chrome,
            status: self.0.as_u16(),
            message: self.1,
        });
        *r.status_mut() = self.0;
        r
    }
}

type PageResult = Result<Response, PageError>;

// ---------------------------------------------------------------------------------------------
// Pagina's
// ---------------------------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    chrome: Chrome,
    products: String,
    available: String,
    price_points: String,
    drops: Vec<ChangeLine>,
    rises: Vec<ChangeLine>,
    has_data: bool,
    scrape_running: bool,
}

pub async fn index(state: State) -> PageResult {
    let stats = db::stats(&state.db).await?;
    let since = Utc::now() - chrono::Duration::days(7);
    let drops = db::recent_changes(&state.db, since, -1, 8).await?;
    let rises = db::recent_changes(&state.db, since, 1, 8).await?;
    let mut chrome = Chrome::new(
        &state,
        "Prijsspoor — prijsgeschiedenis van Collect&Go",
        "home",
    )
    .await;
    chrome.title = "Prijsspoor".into();
    Ok(render(&IndexPage {
        chrome,
        products: fmt_int(stats.products),
        available: fmt_int(stats.available_products),
        price_points: fmt_int(stats.price_points),
        drops: drops.into_iter().map(Into::into).collect(),
        rises: rises.into_iter().map(Into::into).collect(),
        has_data: stats.products > 0,
        scrape_running: state.scrape_lock.is_running(),
    }))
}

#[derive(Deserialize)]
pub struct SearchParams {
    q: Option<String>,
    sort: Option<String>,
    beschikbaar: Option<String>,
    pagina: Option<i64>,
}

struct SortOption {
    value: &'static str,
    label: &'static str,
    selected: bool,
}

struct PageLink {
    number: i64,
    href: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "search.html")]
struct SearchPage {
    chrome: Chrome,
    results: Vec<ProductLine>,
    total: String,
    total_n: i64,
    sorts: Vec<SortOption>,
    only_available: bool,
    pages: Vec<PageLink>,
    prev: Option<String>,
    next: Option<String>,
}

const PER_PAGE: i64 = 30;

pub async fn search(state: State, Query(p): Query<SearchParams>) -> PageResult {
    let q = p.q.unwrap_or_default().trim().to_string();
    // Een geplakte Collect&Go-link of een artikelnummer gaat rechtstreeks naar het product.
    if util::parse_product_url(&q).is_ok() {
        let enc = percent_encoding::utf8_percent_encode(&q, percent_encoding::NON_ALPHANUMERIC);
        return Ok(Redirect::to(&format!("/opzoeken?url={enc}")).into_response());
    }
    if !q.is_empty()
        && q.trim_end_matches("-P").chars().all(|c| c.is_ascii_digit())
        && let Some(id) = db::find_product_id(&state.db, &q).await?
    {
        return Ok(Redirect::to(&format!("/product/{id}")).into_response());
    }

    let sort = Sort::parse(p.sort.as_deref());
    let only_available = p
        .beschikbaar
        .as_deref()
        .is_some_and(|v| v == "1" || v == "on");
    let page = p.pagina.unwrap_or(1).max(1);
    let (rows, total) = db::search(
        &state.db,
        &q,
        sort,
        only_available,
        PER_PAGE,
        (page - 1) * PER_PAGE,
    )
    .await?;

    let pages_n = (total + PER_PAGE - 1) / PER_PAGE;
    let href = |n: i64| {
        let mut s = format!(
            "/zoek?q={}&sort={}",
            percent_encoding::utf8_percent_encode(&q, percent_encoding::NON_ALPHANUMERIC),
            sort.as_str()
        );
        if only_available {
            s.push_str("&beschikbaar=1");
        }
        if n > 1 {
            s.push_str(&format!("&pagina={n}"));
        }
        s
    };
    let window: Vec<i64> = {
        let mut v: Vec<i64> = vec![1, page - 2, page - 1, page, page + 1, page + 2, pages_n];
        v.retain(|&n| n >= 1 && n <= pages_n);
        v.sort();
        v.dedup();
        v
    };
    let sorts = [
        (Sort::Relevance, "Beste match"),
        (Sort::PriceAsc, "Prijs, laag naar hoog"),
        (Sort::PriceDesc, "Prijs, hoog naar laag"),
        (Sort::Name, "Naam"),
        (Sort::RecentlyChanged, "Recent gewijzigd"),
    ]
    .into_iter()
    .map(|(s, label)| SortOption {
        value: s.as_str(),
        label,
        selected: s == sort,
    })
    .collect();

    let mut chrome = Chrome::new(
        &state,
        if q.is_empty() {
            "Alle producten".to_string()
        } else {
            format!("Zoeken naar “{q}”")
        },
        "search",
    )
    .await;
    chrome.q = q.clone();
    Ok(render(&SearchPage {
        chrome,
        results: rows.iter().map(ProductLine::from_row).collect(),
        total: fmt_int(total),
        total_n: total,
        sorts,
        only_available,
        pages: if pages_n > 1 {
            window
                .into_iter()
                .map(|n| PageLink {
                    number: n,
                    href: href(n),
                    current: n == page,
                })
                .collect()
        } else {
            vec![]
        },
        prev: (page > 1).then(|| href(page - 1)),
        next: (page < pages_n).then(|| href(page + 1)),
    }))
}

#[derive(Deserialize)]
pub struct LookupParams {
    url: Option<String>,
}

pub async fn lookup(state: State, Query(p): Query<LookupParams>) -> PageResult {
    let url = p.url.unwrap_or_default();
    match scraper::resolve_url(&state, &url).await {
        Ok(id) => Ok(Redirect::to(&format!("/product/{id}")).into_response()),
        Err(ResolveError::BadUrl(e)) => Err(PageError(StatusCode::BAD_REQUEST, format!("Die link herkennen we niet: {e}."))),
        Err(ResolveError::NotFound) => Err(PageError(
            StatusCode::NOT_FOUND,
            "Op deze link staat geen product dat we kennen. Controleer of de link naar een productpagina op collectandgo.be wijst, of zoek op de productnaam.".into(),
        )),
        Err(ResolveError::Upstream(_)) => Err(PageError(
            StatusCode::BAD_GATEWAY,
            "Collect&Go antwoordt nu niet, dus deze link konden we niet opzoeken. Probeer het over enkele minuten opnieuw, of zoek op de productnaam.".into(),
        )),
        Err(ResolveError::Internal(e)) => Err(e.into()),
    }
}

struct HistoryLine {
    period: String,
    price: String,
    unit_price: Option<String>,
    offer: Option<String>,
    delta: Option<Delta>,
    is_red: bool,
}

#[derive(Template)]
#[template(path = "product.html")]
struct ProductPage {
    chrome: Chrome,
    p: ProductLine,
    long_name: String,
    article_number: String,
    offered_by: String,
    tag: Option<Tag>,
    offer: Option<String>,
    reference_price: Option<String>,
    promos: Vec<String>,
    cogo_url: Option<String>,
    last_updated: Option<String>,
    last_updated_iso: String,
    last_changed: Option<String>,
    unchanged_since_start: bool,
    tracked_since: String,
    stale: bool,
    chart_svg: String,
    chart_json: String,
    history: Vec<HistoryLine>,
    change_count: usize,
    lowest: Option<String>,
    highest: Option<String>,
    api_url: String,
}

pub async fn product(state: State, Path(id): Path<String>) -> PageResult {
    let not_found = || {
        PageError(
            StatusCode::NOT_FOUND,
            "Dit product kennen we niet. Zoek het op naam of plak de Collect&Go-link.".into(),
        )
    };
    let id = db::find_product_id(&state.db, &id)
        .await?
        .ok_or_else(not_found)?;
    let row = db::product(&state.db, &id).await?.ok_or_else(not_found)?;
    let history = db::price_history(&state.db, &id).await?;
    let now = Utc::now();
    let chart = chart::render(&history, now);
    let p = &row.product;

    let history_lines: Vec<HistoryLine> = history
        .iter()
        .enumerate()
        .rev()
        .map(|(i, r)| {
            let until = history
                .get(i + 1)
                .map(|n| n.first_seen_at)
                .unwrap_or(r.last_seen_at);
            HistoryLine {
                period: if i + 1 == history.len() {
                    format!("sinds {}", fmt_date(r.first_seen_at))
                } else {
                    format!("{} tot {}", fmt_date(r.first_seen_at), fmt_date(until))
                },
                price: fmt_euro(r.base_price),
                unit_price: unit_price(r.unit_price, p.unit.as_deref()),
                offer: r.offer_price.map(|o| match r.offer_quantity {
                    Some(q) => format!("€ {} vanaf {q} st.", fmt_euro(o)),
                    None => format!("€ {}", fmt_euro(o)),
                }),
                delta: i
                    .checked_sub(1)
                    .and_then(|j| Delta::new(history[j].base_price, r.base_price)),
                is_red: r.is_red_price,
            }
        })
        .collect();

    let stale = row
        .price_last_seen_at
        .is_some_and(|t| now - t > chrono::Duration::days(2));
    let lowest = history.iter().map(|r| r.base_price).min();
    let highest = history.iter().map(|r| r.base_price).max();
    let mut chrome = Chrome::new(&state, p.name.clone(), "").await;
    chrome.description = match row.base_price {
        Some(c) => format!(
            "{} kost nu € {} bij Collect&Go. Bekijk hoe de prijs evolueerde.",
            p.name,
            fmt_euro(c)
        ),
        None => format!("Prijsgeschiedenis van {} bij Collect&Go.", p.name),
    };

    let page = ProductPage {
        chrome,
        p: ProductLine::from_row(&row),
        long_name: p
            .long_name
            .clone()
            .filter(|l| !l.eq_ignore_ascii_case(&p.name))
            .unwrap_or_default(),
        article_number: p.part_number.trim_end_matches("-P").to_string(),
        offered_by: p.offered_by.clone().unwrap_or_default(),
        tag: row.base_price.map(Tag::new),
        offer: row.offer_price.map(|o| match row.offer_quantity {
            Some(q) => format!("€ {} per stuk vanaf {q} stuks", fmt_euro(o)),
            None => format!("€ {} in aanbieding", fmt_euro(o)),
        }),
        reference_price: row
            .reference_price
            .filter(|r| Some(*r) != row.base_price)
            .map(fmt_euro),
        promos: promotions(&p.promotions),
        cogo_url: site_url(row.url_path.as_deref()),
        last_updated: row.price_last_seen_at.map(fmt_datetime),
        last_updated_iso: row
            .price_last_seen_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_default(),
        last_changed: row.price_first_seen_at.map(fmt_datetime),
        unchanged_since_start: history.len() <= 1,
        tracked_since: fmt_date(p.first_seen_at),
        stale,
        chart_svg: chart.as_ref().map(|c| c.svg.clone()).unwrap_or_default(),
        chart_json: chart
            .map(|c| c.points_json.replace("</", "<\\/"))
            .unwrap_or_else(|| "[]".into()),
        change_count: history.len().saturating_sub(1),
        history: history_lines,
        lowest: lowest.filter(|_| history.len() > 1).map(fmt_euro),
        highest: highest.filter(|_| history.len() > 1).map(fmt_euro),
        api_url: format!("{}/api/v1/products/{}", state.config.public_base_url, p.id),
    };
    Ok(render(&page))
}

#[derive(Deserialize)]
pub struct ChangesParams {
    richting: Option<String>,
    dagen: Option<i64>,
}

struct Choice {
    href: String,
    label: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "changes.html")]
struct ChangesPage {
    chrome: Chrome,
    changes: Vec<ChangeLine>,
    directions: Vec<Choice>,
    periods: Vec<Choice>,
    heading: String,
}

pub async fn changes(state: State, Query(p): Query<ChangesParams>) -> PageResult {
    let days = match p.dagen.unwrap_or(7) {
        d @ (1 | 7 | 30 | 90) => d,
        _ => 7,
    };
    let dir_s = match p.richting.as_deref() {
        Some("omlaag") => "omlaag",
        Some("omhoog") => "omhoog",
        _ => "alle",
    };
    let dir = match dir_s {
        "omlaag" => -1,
        "omhoog" => 1,
        _ => 0,
    };
    let rows = db::recent_changes(
        &state.db,
        Utc::now() - chrono::Duration::days(days),
        dir,
        200,
    )
    .await?;
    let directions = [
        ("alle", "Alle wijzigingen"),
        ("omlaag", "Goedkoper"),
        ("omhoog", "Duurder"),
    ]
    .into_iter()
    .map(|(v, l)| Choice {
        href: format!("/wijzigingen?richting={v}&dagen={days}"),
        label: l.into(),
        current: v == dir_s,
    })
    .collect();
    let periods = [
        (1, "24 uur"),
        (7, "7 dagen"),
        (30, "30 dagen"),
        (90, "90 dagen"),
    ]
    .into_iter()
    .map(|(d, l)| Choice {
        href: format!("/wijzigingen?richting={dir_s}&dagen={d}"),
        label: l.into(),
        current: d == days,
    })
    .collect();
    let heading = match dir {
        -1 => "Goedkoper geworden",
        1 => "Duurder geworden",
        _ => "Prijswijzigingen",
    }
    .to_string();
    Ok(render(&ChangesPage {
        chrome: Chrome::new(&state, heading.clone(), "changes").await,
        changes: rows.into_iter().map(Into::into).collect(),
        directions,
        periods,
        heading,
    }))
}

#[derive(Template)]
#[template(path = "api.html")]
struct ApiPage {
    chrome: Chrome,
    base: String,
    example_url: String,
    example_id: String,
}

pub async fn api_docs(state: State) -> PageResult {
    let example: Option<(String, String)> = sqlx::query_as(
        "SELECT u.path, u.product_id FROM product_urls u JOIN products p ON p.id = u.product_id
         WHERE u.lang = 'nl' AND p.is_available = 1 ORDER BY p.name LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(anyhow::Error::from)?;
    let (example_url, example_id) = match example {
        Some((path, id)) => (site_url(Some(&path)).unwrap_or_default(), id),
        None => (
            "https://www.collectandgo.be/nl/assortiment/boni-walnootpitten-200g".into(),
            "3074457345616678942".into(),
        ),
    };
    Ok(render(&ApiPage {
        chrome: Chrome::new(&state, "API", "api").await,
        base: state.config.public_base_url.clone(),
        example_url,
        example_id,
    }))
}

#[derive(Template)]
#[template(path = "privacy.html")]
struct PrivacyPage {
    chrome: Chrome,
    base: String,
}

pub async fn privacy(state: State) -> PageResult {
    Ok(render(&PrivacyPage {
        chrome: Chrome::new(&state, "Privacy en voorwaarden", "").await,
        base: state.config.public_base_url.clone(),
    }))
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    chrome: Chrome,
    status: u16,
    message: String,
}

pub async fn not_found() -> PageError {
    PageError(
        StatusCode::NOT_FOUND,
        "Deze pagina bestaat niet. Zoek een product hieronder.".into(),
    )
}
