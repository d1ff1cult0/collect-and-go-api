//! bol.com-prijzen, op aanvraag (geen scrape van het hele assortiment).
//!
//! Een bol.com-productpagina bevat schema.org-gegevens (`application/ld+json`) met de prijs van
//! het aanbod in de koopknop. We halen enkel de pagina op die gevraagd wordt, één tegelijk, en
//! bewaren het antwoord een uur. Enkel de Belgische site (`bol.com/be/nl/...`) telt: een link
//! naar `bol.com/nl/...` toont Nederlandse prijzen en is dus fout in een Belgisch budget.

use std::{
    collections::HashMap,
    sync::LazyLock,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::config::Config;

const SITE: &str = "https://www.bol.com";
/// Hoe lang een antwoord van bol.com bewaard blijft.
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
/// Hoogstens zoveel producten in het geheugen.
const CACHE_MAX: usize = 5000;
/// Pauze na elke opvraging, zodat een volle spreadsheet bol.com niet bestookt.
const PAUSE: Duration = Duration::from_millis(400);
/// Hoogstens zoveel opvragingen tegelijk naar bol.com. Klein, maar groot genoeg dat een
/// spreadsheet met veel links binnen de minuut die Google een aanvraag geeft blijft.
const PARALLEL: usize = 2;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BolUrlError {
    #[error("dit is geen bol.com-URL")]
    WrongHost,
    #[error(
        "dit is een link naar bol.com{0}: gebruik de Belgische site (https://www.bol.com/be/nl/...), anders klopt de prijs niet"
    )]
    WrongCountry(String),
    #[error(
        "dit is geen link naar één bol.com-product (verwacht: https://www.bol.com/be/nl/p/<naam>/<nummer>/)"
    )]
    NotAProduct,
}

static PRODUCT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/be/nl/p/[^/]*/(\d{6,20})(?:/|$)").unwrap());

static SITE_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^/([a-z]{2})/([a-z]{2})(?:/|$)").unwrap());

/// Is dit een link naar bol.com (ook zonder schema)?
pub fn is_bol_url(raw: &str) -> bool {
    parse_url(raw).is_ok_and(|u| is_bol_host(&u))
}

fn parse_url(raw: &str) -> Result<url::Url, url::ParseError> {
    let raw = raw.trim();
    if raw.contains("://") {
        url::Url::parse(raw)
    } else {
        url::Url::parse(&format!("https://{raw}"))
    }
}

fn is_bol_host(u: &url::Url) -> bool {
    u.host_str()
        .is_some_and(|h| matches!(h.to_ascii_lowercase().as_str(), "bol.com" | "www.bol.com"))
}

/// Het productnummer uit een bol.com-link. Enkel `bol.com/be/nl/p/...` is goed.
pub fn parse_product_url(raw: &str) -> Result<String, BolUrlError> {
    let u = parse_url(raw).map_err(|_| BolUrlError::WrongHost)?;
    if !is_bol_host(&u) {
        return Err(BolUrlError::WrongHost);
    }
    let path = u.path();
    if let Some(c) = SITE_PREFIX_RE.captures(path) {
        let (land, taal) = (c[1].to_ascii_lowercase(), c[2].to_ascii_lowercase());
        if (land.as_str(), taal.as_str()) != ("be", "nl") {
            return Err(BolUrlError::WrongCountry(format!("/{land}/{taal}")));
        }
    } else {
        // Geen land in de link (bv. bol.com/p/...): bol.com kiest dan zelf, meestal Nederland.
        return Err(BolUrlError::WrongCountry(String::new()));
    }
    PRODUCT_RE
        .captures(path)
        .map(|c| c[1].to_string())
        .ok_or(BolUrlError::NotAProduct)
}

/// Wat we van een productpagina bewaren.
#[derive(Debug, Clone, PartialEq)]
pub struct BolProduct {
    pub id: String,
    pub name: String,
    pub brand: Option<String>,
    pub ean: Option<String>,
    pub image_url: Option<String>,
    /// Prijs in de koopknop, in euro. `None` als er geen aanbod is.
    pub price: Option<f64>,
    pub currency: String,
    pub is_available: bool,
    pub seller: Option<String>,
}

static LD_JSON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<script[^>]*type\s*=\s*["']application/ld\+json["'][^>]*>(.*?)</script>"#)
        .unwrap()
});

fn text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Object(o) => text(o.get("name")),
        Value::Array(a) => text(a.first()),
        _ => None,
    }
}

/// Een afbeelding: een URL, een `ImageObject` of een lijst daarvan.
fn image(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if s.starts_with("http") => Some(s.clone()),
        Value::Object(o) => image(o.get("url")).or_else(|| image(o.get("contentUrl"))),
        Value::Array(a) => a.iter().find_map(|x| image(Some(x))),
        _ => None,
    }
}

fn number(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().replace(',', ".").parse().ok(),
        _ => None,
    }
}

fn is_type(v: &Value, t: &str) -> bool {
    match v.get("@type") {
        Some(Value::String(s)) => s.eq_ignore_ascii_case(t),
        Some(Value::Array(a)) => a
            .iter()
            .any(|x| x.as_str().is_some_and(|s| s.eq_ignore_ascii_case(t))),
        _ => false,
    }
}

/// Alle objecten in een JSON-LD-blok: het blok zelf, een lijst, of een `@graph`.
fn objects(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().flat_map(objects).collect(),
        Value::Object(o) => {
            let mut out = vec![v];
            if let Some(g) = o.get("@graph") {
                out.extend(objects(g));
            }
            out
        }
        _ => vec![],
    }
}

/// Leest het product uit de HTML van een bol.com-productpagina.
pub fn parse_product_page(html: &str) -> Option<BolProduct> {
    let blocks: Vec<Value> = LD_JSON_RE
        .captures_iter(html)
        .filter_map(|c| serde_json::from_str(c[1].trim()).ok())
        .collect();
    let product = blocks
        .iter()
        .flat_map(objects)
        .find(|o| is_type(o, "Product"))?;

    // Het aanbod: één Offer, een lijst Offers (de eerste is de koopknop), of een AggregateOffer.
    let offer = match product.get("offers") {
        Some(Value::Array(a)) => a.first(),
        other => other,
    };
    let (price, currency, available, seller) = match offer {
        Some(o) => {
            let price = number(o.get("price")).or_else(|| number(o.get("lowPrice")));
            let availability = text(o.get("availability")).unwrap_or_default();
            let available = price.is_some()
                && !["OutOfStock", "SoldOut", "Discontinued"]
                    .iter()
                    .any(|x| availability.ends_with(x));
            (
                price,
                text(o.get("priceCurrency")).unwrap_or_else(|| "EUR".into()),
                available,
                text(o.get("seller")),
            )
        }
        None => (None, "EUR".into(), false, None),
    };

    Some(BolProduct {
        id: text(product.get("productID")).or_else(|| text(product.get("sku")))?,
        name: text(product.get("name"))?,
        brand: text(product.get("brand")),
        ean: text(product.get("gtin13")).or_else(|| text(product.get("gtin"))),
        image_url: image(product.get("image")),
        price,
        currency,
        is_available: available,
        seller,
    })
}

// ---------------------------------------------------------------------------------------------
// Ophalen
// ---------------------------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum BolError {
    #[error(transparent)]
    BadUrl(#[from] BolUrlError),
    #[error("product niet gevonden op bol.com")]
    NotFound,
    #[error("bol.com is momenteel niet bereikbaar: {0}")]
    Upstream(String),
}

#[derive(Clone)]
struct Cached {
    at: Instant,
    seen_at: DateTime<Utc>,
    /// `None`: bol.com kent dit product niet (404 of geen productgegevens).
    product: Option<BolProduct>,
}

pub struct BolClient {
    /// `https://www.bol.com`; enkel in tests anders.
    site: String,
    http: reqwest::Client,
    relay: Option<(String, String, reqwest::Client)>,
    cache: std::sync::Mutex<HashMap<String, Cached>>,
    /// Beperkt het aantal opvragingen tegelijk naar bol.com.
    permits: Semaphore,
}

/// Een gewone browser stuurt deze headers mee; zonder antwoordt bol.com met 403.
const BROWSER_HEADERS: &[(&str, &str)] = &[
    (
        "Accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    ),
    ("Accept-Language", "nl-BE,nl;q=0.9,en;q=0.8"),
    (
        "sec-ch-ua",
        r#""Chromium";v="142", "Google Chrome";v="142", "Not_A Brand";v="99""#,
    ),
    ("sec-ch-ua-mobile", "?0"),
    ("sec-ch-ua-platform", r#""macOS""#),
    ("sec-fetch-dest", "document"),
    ("sec-fetch-mode", "navigate"),
    ("sec-fetch-site", "none"),
    ("sec-fetch-user", "?1"),
    ("upgrade-insecure-requests", "1"),
];

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";

impl BolClient {
    pub fn new(cfg: &Config) -> Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        for (k, v) in BROWSER_HEADERS {
            headers.insert(*k, v.parse()?);
        }
        let mut builder = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .gzip(true)
            .brotli(true);
        if let Some(proxy) = &cfg.proxy {
            builder = builder
                .proxy(reqwest::Proxy::all(proxy).context("COGO_PROXY is geen geldige proxy-URL")?);
        }
        let relay = match &cfg.relay {
            Some(r) => Some((
                r.url.trim_end_matches('/').to_string(),
                r.secret.clone(),
                reqwest::Client::builder()
                    .gzip(true)
                    .timeout(Duration::from_secs(90))
                    .build()?,
            )),
            None => None,
        };
        Ok(Self {
            site: SITE.to_string(),
            http: builder.build()?,
            relay,
            cache: Default::default(),
            permits: Semaphore::new(PARALLEL),
        })
    }

    fn cached(&self, id: &str) -> Option<Cached> {
        let cache = self.cache.lock().ok()?;
        cache
            .get(id)
            .filter(|c| c.at.elapsed() < CACHE_TTL)
            .cloned()
    }

    fn store(&self, id: &str, product: Option<BolProduct>) -> Cached {
        let entry = Cached {
            at: Instant::now(),
            seen_at: Utc::now(),
            product,
        };
        if let Ok(mut cache) = self.cache.lock() {
            if cache.len() >= CACHE_MAX {
                cache.retain(|_, c| c.at.elapsed() < CACHE_TTL);
            }
            if cache.len() < CACHE_MAX {
                cache.insert(id.to_string(), entry.clone());
            }
        }
        entry
    }

    /// Het product achter een bol.com-link, met het moment waarop de prijs gezien werd.
    pub async fn product(&self, raw: &str) -> Result<(BolProduct, DateTime<Utc>), BolError> {
        let id = parse_product_url(raw)?;
        let entry = match self.cached(&id) {
            Some(c) => c,
            None => {
                let _permit = self
                    .permits
                    .acquire()
                    .await
                    .map_err(|_| BolError::Upstream("afgesloten".into()))?;
                // Misschien haalde een andere aanvraag hem net op.
                match self.cached(&id) {
                    Some(c) => c,
                    None => {
                        let found = self.fetch(&id).await;
                        tokio::time::sleep(PAUSE).await;
                        self.store(&id, found?)
                    }
                }
            }
        };
        let seen_at = entry.seen_at;
        entry
            .product
            .map(|p| (p, seen_at))
            .ok_or(BolError::NotFound)
    }

    /// Haalt de pagina op: eerst rechtstreeks, bij een blokkade via de relay (als die er is).
    async fn fetch(&self, id: &str) -> Result<Option<BolProduct>, BolError> {
        let url = format!("{}/be/nl/p/-/{id}/", self.site);
        let (mut status, mut body) = self
            .direct(&url)
            .await
            .map_err(|e| BolError::Upstream(format!("{e:#}")))?;
        if matches!(status, 403 | 429 | 456)
            && let Some(relay) = &self.relay
        {
            tracing::warn!(
                status,
                "bol.com houdt de rechtstreekse aanvraag tegen, via de relay"
            );
            (status, body) = Self::via_relay(relay, &url)
                .await
                .map_err(|e| BolError::Upstream(format!("{e:#}")))?;
        }
        match status {
            200..300 => {
                let html = String::from_utf8_lossy(&body);
                Ok(parse_product_page(&html).filter(|p| p.id == id))
            }
            404 | 410 => Ok(None),
            403 | 429 | 456 => Err(BolError::Upstream(format!(
                "bol.com houdt de aanvraag tegen (HTTP {status})"
            ))),
            _ => Err(BolError::Upstream(format!("HTTP {status}"))),
        }
    }

    async fn direct(&self, url: &str) -> Result<(u16, bytes::Bytes)> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("request naar {url} mislukt"))?;
        let status = resp.status().as_u16();
        Ok((status, resp.bytes().await.context("body lezen mislukt")?))
    }

    async fn via_relay(
        (base, secret, http): &(String, String, reqwest::Client),
        url: &str,
    ) -> Result<(u16, bytes::Bytes)> {
        let resp = http
            .post(format!("{base}/fetch"))
            .header("x-relay-secret", secret)
            .json(&serde_json::json!({ "url": url, "accept": "text/html" }))
            .send()
            .await
            .context("relay niet bereikbaar")?;
        if !resp.status().is_success() {
            bail!("relay antwoordde HTTP {}", resp.status().as_u16());
        }
        let status = resp
            .headers()
            .get("x-relay-status")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| anyhow!("relay gaf geen x-relay-status"))?;
        Ok((status, resp.bytes().await?))
    }
}

// ---------------------------------------------------------------------------------------------
// JSON voor de API: dezelfde vorm als bij Collect&Go, zodat de Sheets-add-on beide gelijk leest.
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct BolProductJson {
    pub id: String,
    pub name: String,
    pub brand: Option<String>,
    pub ean: Option<String>,
    pub image_url: Option<String>,
    pub url: String,
    pub is_available: bool,
    pub seller: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BolPriceJson {
    pub currency: String,
    pub price: f64,
    /// bol.com kent geen hoeveelheidskorting zoals Collect&Go; altijd leeg.
    pub offer: Option<()>,
}

#[derive(Debug, Serialize)]
pub struct BolProductPriceJson {
    pub source: &'static str,
    pub product: BolProductJson,
    pub price: Option<BolPriceJson>,
    pub last_updated_at: DateTime<Utc>,
}

impl BolProductPriceJson {
    pub fn new(p: BolProduct, seen_at: DateTime<Utc>) -> Self {
        Self {
            source: "bol.com",
            price: p.price.map(|price| BolPriceJson {
                currency: p.currency.clone(),
                price,
                offer: None,
            }),
            product: BolProductJson {
                url: format!("{SITE}/be/nl/p/-/{}/", p.id),
                id: p.id,
                name: p.name,
                brand: p.brand,
                ean: p.ean,
                image_url: p.image_url,
                is_available: p.is_available,
                seller: p.seller,
            },
            last_updated_at: seen_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            parse_product_url(
                "https://www.bol.com/be/nl/p/brabantia-profile-roerlepel-beukenhout/9200000130662509/"
            ),
            Ok("9200000130662509".into())
        );
        assert_eq!(
            parse_product_url("bol.com/be/nl/p/-/9200000130662509?bltgh=abc#x"),
            Ok("9200000130662509".into())
        );
        assert_eq!(
            parse_product_url("https://www.bol.com/BE/NL/p/x/9200000130662509/"),
            Err(BolUrlError::NotAProduct),
            "het pad is hoofdlettergevoelig bij bol.com"
        );
        assert_eq!(
            parse_product_url(
                "https://www.bol.com/nl/nl/p/brabantia-profile-roerlepel-beukenhout/9200000130662509/"
            ),
            Err(BolUrlError::WrongCountry("/nl/nl".into()))
        );
        assert_eq!(
            parse_product_url("https://www.bol.com/be/fr/p/x/9200000130662509/"),
            Err(BolUrlError::WrongCountry("/be/fr".into()))
        );
        assert_eq!(
            parse_product_url("https://www.bol.com/p/x/9200000130662509/"),
            Err(BolUrlError::WrongCountry(String::new()))
        );
        assert_eq!(
            parse_product_url("https://www.bol.com/be/nl/s/?searchtext=lepels"),
            Err(BolUrlError::NotAProduct)
        );
        assert_eq!(
            parse_product_url("https://evilbol.com/be/nl/p/x/9200000130662509/"),
            Err(BolUrlError::WrongHost)
        );
        assert!(is_bol_url("https://www.bol.com/nl/nl/p/x/1/"));
        assert!(!is_bol_url("https://www.collectandgo.be/nl/assortiment/x"));
    }

    const PAGE: &str = r#"<html><head>
<script type="application/ld+json">{"@type":"Product","@id":"https://www.bol.com/#9200000130662509","name":"Brabantia Profile Roerlepel - Beukenhout","productID":"9200000130662509","gtin13":"8710755260582","brand":{"@type":"Brand","name":"Brabantia"},"image":{"@type":"ImageObject","url":"https://media.s-bol.com/x.jpg","width":278},"offers":{"@type":"Offer","price":"10.88","priceCurrency":"EUR","availability":"InStock","seller":{"@type":"Organization","name":"bol"}}}</script>
<script type="application/ld+json">{"@type":"BreadcrumbList","itemListElement":[]}</script>
</head></html>"#;

    #[test]
    fn product_page() {
        let p = parse_product_page(PAGE).unwrap();
        assert_eq!(p.id, "9200000130662509");
        assert_eq!(p.name, "Brabantia Profile Roerlepel - Beukenhout");
        assert_eq!(p.brand.as_deref(), Some("Brabantia"));
        assert_eq!(p.ean.as_deref(), Some("8710755260582"));
        assert_eq!(p.price, Some(10.88));
        assert!(p.is_available);
        assert_eq!(p.seller.as_deref(), Some("bol"));
        assert_eq!(
            p.image_url.as_deref(),
            Some("https://media.s-bol.com/x.jpg")
        );

        let uitverkocht = PAGE
            .replace(r#""price":"10.88","#, "")
            .replace("InStock", "https://schema.org/OutOfStock");
        let p = parse_product_page(&uitverkocht).unwrap();
        assert_eq!(p.price, None);
        assert!(!p.is_available);

        assert_eq!(parse_product_page("<html>geen product</html>"), None);
    }

    /// bol.com weigert de server (403), de relay krijgt de pagina wel.
    #[tokio::test]
    async fn falls_back_to_relay_and_caches() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        use axum::{
            Router,
            http::{HeaderMap, StatusCode},
            routing::{get, post},
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let direct = Router::new().fallback(get(|| async { (StatusCode::FORBIDDEN, "nee") }));
        let teller = calls.clone();
        let relay = Router::new().route(
            "/fetch",
            post(move |body: String| {
                let teller = teller.clone();
                async move {
                    teller.fetch_add(1, Ordering::SeqCst);
                    assert!(body.contains("/be/nl/p/-/9200000130662509/"), "{body}");
                    let mut h = HeaderMap::new();
                    h.insert("x-relay-status", "200".parse().unwrap());
                    (StatusCode::OK, h, PAGE)
                }
            }),
        );
        let mut urls = Vec::new();
        for app in [direct, relay] {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            urls.push(format!("http://{}", l.local_addr().unwrap()));
            tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        }
        let mut cfg = Config::from_env().unwrap();
        cfg.proxy = None;
        cfg.relay = Some(crate::config::RelayConfig {
            url: urls[1].clone(),
            secret: "0123456789abcdef".into(),
            always: false,
        });
        let mut client = BolClient::new(&cfg).unwrap();
        client.site = urls[0].clone();

        let link =
            "https://www.bol.com/be/nl/p/brabantia-profile-roerlepel-beukenhout/9200000130662509/";
        let (p, _) = client.product(link).await.unwrap();
        assert_eq!(p.price, Some(10.88));
        // Tweede keer uit de cache, zonder bol.com of de relay.
        client.product(link).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Een link naar de Nederlandse site wordt niet eens opgevraagd.
        assert!(matches!(
            client
                .product("https://www.bol.com/nl/nl/p/x/9200000130662509/")
                .await,
            Err(BolError::BadUrl(BolUrlError::WrongCountry(_)))
        ));
    }
}
