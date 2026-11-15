//! Client voor Collect&Go (www.collectandgo.be + de API-gateway apip.collectandgo.be).
//!
//! De website laadt productlijsten via de HCL Commerce "productview"-zoekdienst achter
//! `apip.collectandgo.be`. Die geeft per product de actuele prijs (`xprice`) terug, in pagina's
//! van maximaal ~1000 producten. `bySearchTerm/*` levert het volledige assortiment.

use std::{
    sync::LazyLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use serde::{Deserialize, Deserializer};
use tokio::sync::{Mutex, RwLock};

use crate::{config::Config, util};

pub const SITE: &str = "https://www.collectandgo.be";
pub const IMAGE_BASE: &str = "https://images.collectandgo.be/images/step/JPG/";
const GATEWAY: &str =
    "https://apip.collectandgo.be/gateway/ecomfoodb2c.eshop.wcsproductviewsearchsvc/v1";
const STORE_ID: &str = "90004";
const CATALOG_ID: &str = "10429";
const LANG_ID: &str = "-1001"; // Nederlands
const MAX_ATTEMPTS: u32 = 6;

// ---------------------------------------------------------------------------------------------
// Ruwe API-types
// ---------------------------------------------------------------------------------------------

fn num_or_str<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    #[serde(rename = "recordSetTotal", default, deserialize_with = "num_or_str")]
    pub total: Option<u64>,
    #[serde(rename = "catalogEntryView", default)]
    pub entries: Vec<CatalogEntry>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    #[serde(rename = "uniqueID")]
    pub unique_id: String,
    #[serde(default)]
    pub part_number: String,
    #[serde(rename = "x_techArticleId", default)]
    pub tech_article_id: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub product_long_name: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub full_image: Option<String>,
    #[serde(default)]
    pub is_product_available: Option<String>,
    #[serde(default)]
    pub is_product_ranged: Option<String>,
    #[serde(default)]
    pub xprice: Vec<XPrice>,
    #[serde(default)]
    pub promotions: Vec<RawPromotion>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
    /// Enkel bij `byId`: de SKU's van het product (prijs zit dan soms enkel daar).
    #[serde(rename = "sKUs", default)]
    pub skus: Vec<CatalogEntry>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct XPrice {
    pub base_price: Option<String>,
    pub base_price_vol: Option<String>,
    pub offer_price: Option<String>,
    pub offer_price_qty: Option<String>,
    pub offer_price_vol: Option<String>,
    pub pid_price: Option<String>,
    pub red_flag: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RawPromotion {
    pub strap_line: Option<String>,
    pub second_strap_line: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Attribute {
    pub identifier: Option<String>,
    #[serde(default)]
    pub values: Vec<AttributeValue>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AttributeValue {
    pub value: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Genormaliseerde types
// ---------------------------------------------------------------------------------------------

/// Prijsinformatie zoals waargenomen; bedragen in eurocent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceObs {
    pub base_price: i64,
    pub unit_price: Option<i64>,
    pub offer_price: Option<i64>,
    pub offer_quantity: Option<i64>,
    pub offer_unit_price: Option<i64>,
    pub reference_price: Option<i64>,
    pub is_red_price: bool,
}

#[derive(Debug, Clone)]
pub struct ScrapedProduct {
    pub id: String,
    pub part_number: String,
    pub tech_article_id: Option<String>,
    pub name: String,
    pub long_name: Option<String>,
    pub brand: Option<String>,
    pub content: Option<String>,
    pub unit: Option<String>,
    pub image_path: Option<String>,
    pub image_asset: Option<String>,
    pub offered_by: Option<String>,
    pub is_available: bool,
    pub is_ranged: bool,
    pub promotions: Vec<String>,
    pub price: Option<PriceObs>,
}

static ASSET_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"asset-(\d+)").unwrap());

pub fn image_asset(path: &str) -> Option<String> {
    ASSET_RE.captures(path).map(|c| c[1].to_string())
}

/// "3,19" -> 319 cent.
pub fn parse_cents(s: &str) -> Option<i64> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return None;
    }
    let v: f64 = s.parse().ok()?;
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    Some((v * 100.0).round() as i64)
}

impl CatalogEntry {
    fn attr(&self, id: &str) -> Option<String> {
        self.attributes
            .iter()
            .find(|a| a.identifier.as_deref() == Some(id))
            .and_then(|a| a.values.iter().find_map(|v| v.value.clone()))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    fn price_obs(&self) -> Option<PriceObs> {
        let xp = self
            .xprice
            .first()
            .or_else(|| self.skus.iter().find_map(|s| s.xprice.first()))?;
        let base_price = parse_cents(xp.base_price.as_deref()?)?;
        let opt = |v: &Option<String>| v.as_deref().and_then(parse_cents);
        let offer_price = opt(&xp.offer_price);
        Some(PriceObs {
            base_price,
            unit_price: opt(&xp.base_price_vol),
            offer_price,
            offer_quantity: offer_price.and(
                xp.offer_price_qty
                    .as_deref()
                    .and_then(|q| q.trim().parse::<f64>().ok())
                    .map(|q| q.round() as i64),
            ),
            offer_unit_price: offer_price.and(opt(&xp.offer_price_vol)),
            reference_price: opt(&xp.pid_price),
            is_red_price: xp.red_flag.as_deref() == Some("Y"),
        })
    }

    pub fn normalize(&self) -> ScrapedProduct {
        let image_path = self
            .thumbnail
            .clone()
            .or_else(|| self.full_image.clone())
            .or_else(|| {
                self.skus
                    .iter()
                    .find_map(|s| s.full_image.clone().or(s.thumbnail.clone()))
            });
        let mut promotions: Vec<String> = Vec::new();
        for p in self
            .promotions
            .iter()
            .chain(self.skus.iter().flat_map(|s| s.promotions.iter()))
        {
            let mut line = p.strap_line.clone().unwrap_or_default().trim().to_string();
            if let Some(second) = p.second_strap_line.as_deref().map(str::trim)
                && !second.is_empty()
            {
                let second = second.trim_start_matches(',').trim();
                if line.is_empty() {
                    line = second.to_string();
                } else {
                    line = format!("{line} ({second})");
                }
            }
            if !line.is_empty() && !promotions.contains(&line) {
                promotions.push(line);
            }
        }
        let yes = |v: &Option<String>| v.as_deref() == Some("Y");
        let first_sku = self.skus.first();
        ScrapedProduct {
            id: self.unique_id.clone(),
            part_number: self.part_number.clone(),
            tech_article_id: self.tech_article_id.clone(),
            name: self.name.trim().to_string(),
            long_name: self.product_long_name.clone().map(|s| s.trim().to_string()),
            brand: self.attr("SEOBrandName").or_else(|| self.attr("BrandName")),
            content: self
                .attr("SEOContent")
                .or_else(|| self.attr("LongNameContent"))
                .filter(|c| {
                    c.chars()
                        .any(|ch| ch.is_alphabetic() || ('1'..='9').contains(&ch))
                }),
            unit: self.attr("NetWeightWithoutFreePart"),
            image_asset: image_path.as_deref().and_then(image_asset),
            image_path,
            offered_by: self.attr("AvailableBy"),
            is_available: yes(&self.is_product_available)
                || first_sku.is_some_and(|s| yes(&s.is_product_available)),
            is_ranged: yes(&self.is_product_ranged)
                || first_sku.is_some_and(|s| yes(&s.is_product_ranged)),
            promotions,
            price: self.price_obs(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Sitemap & productpagina
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SitemapEntry {
    /// Alle URL's (nl + alternatieven) van dit product.
    pub urls: Vec<String>,
    pub image_asset: Option<String>,
    pub title: Option<String>,
}

static SM_URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<url>(.*?)</url>").unwrap());
static SM_LOC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<loc>\s*([^<\s]+)\s*</loc>").unwrap());
static SM_ALT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<xhtml:link[^>]*href="([^"]+)""#).unwrap());
static SM_IMG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<image:loc[^>]*>(.*?)</image:loc>").unwrap());
static SM_TITLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<image:title[^>]*>(.*?)</image:title>").unwrap());

pub fn parse_sitemap_index(xml: &str) -> Vec<String> {
    let re = Regex::new(r"(?s)<sitemap>.*?<loc>\s*([^<\s]+)\s*</loc>").unwrap();
    re.captures_iter(xml).map(|c| c[1].to_string()).collect()
}

pub fn parse_product_sitemap(xml: &str) -> Vec<SitemapEntry> {
    SM_URL_RE
        .captures_iter(xml)
        .filter_map(|c| {
            let block = &c[1];
            let loc = SM_LOC_RE.captures(block)?[1].to_string();
            let mut urls = vec![loc];
            for alt in SM_ALT_RE.captures_iter(block) {
                let u = util::xml_unescape(&alt[1]);
                if !urls.contains(&u) {
                    urls.push(u);
                }
            }
            Some(SitemapEntry {
                urls,
                image_asset: SM_IMG_RE.captures(block).and_then(|m| image_asset(&m[1])),
                title: SM_TITLE_RE
                    .captures(block)
                    .map(|m| util::xml_unescape(m[1].trim()))
                    .filter(|t| !t.is_empty()),
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct ProductPage {
    pub product_id: String,
    /// Canonical + hreflang-alternatieven.
    pub urls: Vec<String>,
}

static PAGE_ENTITLED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"id="entitledItem_(\d+)""#).unwrap());
static PAGE_UNIQUE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""uniqueID":"(\d+)","partNumber":"[^"]*-P""#).unwrap());
static PAGE_ALT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<link\s+rel="(?:alternate|canonical)"[^>]*href="(https://www\.collectandgo\.be/(?:nl|fr)/assortiment/[^"]+)""#)
        .unwrap()
});

pub fn parse_product_page(html: &str) -> Option<ProductPage> {
    let product_id = PAGE_ENTITLED_RE
        .captures(html)
        .or_else(|| PAGE_UNIQUE_RE.captures(html))
        .map(|c| c[1].to_string())?;
    let mut urls = Vec::new();
    for c in PAGE_ALT_RE.captures_iter(html) {
        let u = util::xml_unescape(&c[1]);
        if !urls.contains(&u) {
            urls.push(u);
        }
    }
    Some(ProductPage { product_id, urls })
}

static API_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""apiGatewayKey"\s*:\s*['"]([0-9a-fA-F-]{20,})['"]"#).unwrap());

// ---------------------------------------------------------------------------------------------
// HTTP-client
// ---------------------------------------------------------------------------------------------

pub struct CogoClient {
    http: reqwest::Client,
    api_key: RwLock<String>,
    request_delay: Duration,
    /// Serialiseert requests naar de website zelf (productpagina's), zodat de publieke API
    /// nooit een stortvloed aan requests naar Collect&Go kan veroorzaken.
    site_lock: Mutex<()>,
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("niet gevonden (404)")]
    NotFound,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

fn jitter(max_ms: u64) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    Duration::from_millis(u64::from(nanos) % max_ms.max(1))
}

impl CogoClient {
    pub fn new(cfg: &Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(cfg.user_agent.clone())
            .timeout(Duration::from_secs(90))
            .connect_timeout(Duration::from_secs(20))
            .gzip(true)
            .brotli(true)
            .build()?;
        Ok(Self {
            http,
            api_key: RwLock::new(cfg.api_key.clone()),
            request_delay: cfg.request_delay,
            site_lock: Mutex::new(()),
        })
    }

    pub fn request_delay(&self) -> Duration {
        self.request_delay
    }

    /// GET met retries + exponentiële backoff. Geeft de body terug.
    async fn get_with_retry(
        &self,
        url: &str,
        use_api_key: bool,
        accept: &str,
    ) -> Result<String, FetchError> {
        let mut last_err = anyhow!("geen poging gedaan");
        let mut refreshed_key = false;
        for attempt in 1..=MAX_ATTEMPTS {
            let mut req = self
                .http
                .get(url)
                .header("Accept", accept)
                .header("Accept-Language", "nl-BE,nl;q=0.9")
                .header("Referer", format!("{SITE}/"));
            if use_api_key {
                req = req
                    .header("X-CG-APIKey", self.api_key.read().await.clone())
                    .header("Origin", SITE);
            }
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        match resp.text().await {
                            Ok(body) => return Ok(body),
                            Err(e) => last_err = anyhow!(e).context("body lezen mislukt"),
                        }
                    } else if status == reqwest::StatusCode::NOT_FOUND {
                        return Err(FetchError::NotFound);
                    } else if use_api_key && !refreshed_key && matches!(status.as_u16(), 401 | 403)
                    {
                        refreshed_key = true;
                        tracing::warn!(%status, "API-key geweigerd, nieuwe key ophalen van de website");
                        if let Err(e) = self.refresh_api_key().await {
                            tracing::warn!(error = %e, "API-key vernieuwen mislukt");
                        }
                        last_err = anyhow!("HTTP {status}");
                        continue;
                    } else {
                        last_err = anyhow!("HTTP {status} voor {url}");
                        if status.is_client_error()
                            && !matches!(status.as_u16(), 408 | 425 | 429 | 456)
                        {
                            return Err(FetchError::Other(last_err));
                        }
                    }
                }
                Err(e) => last_err = anyhow!(e).context(format!("request naar {url} mislukt")),
            }
            if attempt < MAX_ATTEMPTS {
                // 2s, 4s, 8s, 16s, 32s (+ jitter); 456 = anti-bot, dus ruim wachten.
                let backoff = Duration::from_secs(2u64.pow(attempt)) + jitter(1500);
                tracing::warn!(attempt, error = %last_err, ?backoff, "request mislukt, opnieuw proberen");
                tokio::time::sleep(backoff).await;
            }
        }
        Err(FetchError::Other(last_err))
    }

    async fn refresh_api_key(&self) -> Result<()> {
        let html = self
            .http
            .get(format!("{SITE}/nl/home"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let key = API_KEY_RE
            .captures(&html)
            .map(|c| c[1].to_string())
            .context("apiGatewayKey niet gevonden op de homepage")?;
        tracing::info!("nieuwe API-key gevonden");
        *self.api_key.write().await = key;
        Ok(())
    }

    /// Eén pagina (1-based) uit het volledige assortiment.
    pub async fn search_page(&self, page: u32, page_size: u32) -> Result<SearchResponse> {
        let url = format!(
            "{GATEWAY}/store/{STORE_ID}/productview/bySearchTerm/*?searchSource=E&shopName=CLPBE\
             &pageSize={page_size}&pageNumber={page}&orderBy=1&catalogId={CATALOG_ID}&langId={LANG_ID}"
        );
        let body = self
            .get_with_retry(&url, true, "application/json")
            .await
            .map_err(|e| anyhow!(e))?;
        serde_json::from_str(&body).context("zoekresultaat kon niet geparsed worden")
    }

    /// Eén product via zijn uniqueID.
    pub async fn product_by_id(&self, id: &str) -> Result<Option<CatalogEntry>> {
        anyhow::ensure!(
            id.chars().all(|c| c.is_ascii_digit()),
            "ongeldig product-ID"
        );
        let url = format!(
            "{GATEWAY}/store/{STORE_ID}/productview/byId/{id}?langId={LANG_ID}&catalogId={CATALOG_ID}"
        );
        match self.get_with_retry(&url, true, "application/json").await {
            Ok(body) => {
                let resp: SearchResponse =
                    serde_json::from_str(&body).context("byId kon niet geparsed worden")?;
                Ok(resp.entries.into_iter().next())
            }
            Err(FetchError::NotFound) => Ok(None),
            Err(FetchError::Other(e)) => Err(e),
        }
    }

    pub async fn sitemap_product_urls(&self) -> Result<Vec<String>> {
        let xml = self
            .get_with_retry(&format!("{SITE}/sitemap.xml"), false, "application/xml")
            .await
            .map_err(|e| anyhow!(e))?;
        let all = parse_sitemap_index(&xml);
        let nl: Vec<String> = all
            .iter()
            .filter(|u| u.contains("nl_BE-product"))
            .cloned()
            .collect();
        if nl.is_empty() {
            bail!("geen product-sitemaps gevonden in sitemap.xml");
        }
        Ok(nl)
    }

    pub async fn sitemap(&self, url: &str) -> Result<Vec<SitemapEntry>> {
        let resp = self
            .http
            .get(url)
            .header("Referer", format!("{SITE}/"))
            .send()
            .await?
            .error_for_status()?;
        let bytes = resp.bytes().await?;
        let xml = if bytes.starts_with(&[0x1f, 0x8b]) {
            use std::io::Read;
            let mut s = String::new();
            flate2::read::GzDecoder::new(&bytes[..]).read_to_string(&mut s)?;
            s
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };
        Ok(parse_product_sitemap(&xml))
    }

    /// Haalt een productpagina op en leest het product-ID eruit.
    pub async fn product_page(&self, path: &str) -> Result<Option<ProductPage>, FetchError> {
        let _guard = self.site_lock.lock().await;
        let url = format!("{SITE}{path}");
        let html = self.get_with_retry(&url, false, "text/html").await?;
        let page = parse_product_page(&html);
        // Kleine pauze zodat opeenvolgende opzoekingen de site niet bestoken.
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cents() {
        assert_eq!(parse_cents("3,19"), Some(319));
        assert_eq!(parse_cents("15,95"), Some(1595));
        assert_eq!(parse_cents("0,1"), Some(10));
        assert_eq!(parse_cents("12"), Some(1200));
        assert_eq!(parse_cents(""), None);
        assert_eq!(parse_cents("abc"), None);
    }

    #[test]
    fn normalize_entry() {
        let json = r#"{"uniqueID":"3074457345616678942","partNumber":"28856-P","x_techArticleId":"28856",
          "name":"BONI walnootpitten 200g","thumbnail":"JPG/200x200/std.lang.all/62/17/asset-3916217.jpg",
          "isProductAvailable":"Y","isProductRanged":"Y",
          "xprice":[{"basePriceVol":"15,95","offerPrice":"3,06","offerPriceQty":"3","offerPriceVol":"15,30","redFlag":"N","basePrice":"3,19"}],
          "promotions":[{"strapLine":"-0,55€ bij aankoop van 1 st","secondStrapLine":",vanaf 04/11/26 tot en met 17/11/26"}],
          "attributes":[{"identifier":"SEOBrandName","values":[{"value":"Boni"}]},
                        {"identifier":"SEOContent","values":[{"value":"200g"}]},
                        {"identifier":"NetWeightWithoutFreePart","values":[{"value":"kg"}]}]}"#;
        let e: CatalogEntry = serde_json::from_str(json).unwrap();
        let p = e.normalize();
        assert_eq!(p.id, "3074457345616678942");
        assert_eq!(p.brand.as_deref(), Some("Boni"));
        assert_eq!(p.content.as_deref(), Some("200g"));
        assert_eq!(p.unit.as_deref(), Some("kg"));
        assert_eq!(p.image_asset.as_deref(), Some("3916217"));
        assert!(p.is_available && p.is_ranged);
        assert_eq!(
            p.promotions,
            vec!["-0,55€ bij aankoop van 1 st (vanaf 04/11/26 tot en met 17/11/26)"]
        );
        let price = p.price.unwrap();
        assert_eq!(price.base_price, 319);
        assert_eq!(price.unit_price, Some(1595));
        assert_eq!(price.offer_price, Some(306));
        assert_eq!(price.offer_quantity, Some(3));
        assert_eq!(price.offer_unit_price, Some(1530));
        assert_eq!(price.reference_price, None);
        assert!(!price.is_red_price);
    }

    #[test]
    fn sitemap_parsing() {
        let xml = r#"<urlset><url>
  <loc>https://www.collectandgo.be/nl/assortiment/c%C3%B4te-dor-noir-%7C-orange-240g</loc>
  <lastmod>2026-04-03</lastmod>
  <xhtml:link href="https://www.collectandgo.be/nl/assortiment/c%C3%B4te-dor-noir-%7C-orange-240g" hreflang="nl_BE" rel="alternate"/>
  <xhtml:link href="https://www.collectandgo.be/fr/assortiment/c%C3%B4te-dor-noir%7C-orange-240g" hreflang="fr_FR" rel="alternate"/>
<image:image><image:title xml:space='preserve'>CÔTE D&apos;OR Mignonette Noir Orange 240g </image:title><image:loc xml:space='preserve'>https://images.collectandgo.be/images/step/JPG/JPG/500x500/std.lang.all/46/36/asset-4114636.jpg </image:loc></image:image><priority>0.9</priority></url></urlset>"#;
        let e = parse_product_sitemap(xml);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].urls.len(), 2);
        assert_eq!(e[0].image_asset.as_deref(), Some("4114636"));
        assert_eq!(
            e[0].title.as_deref(),
            Some("CÔTE D'OR Mignonette Noir Orange 240g")
        );
    }

    #[test]
    fn product_page_parsing() {
        let html = r#"<link rel="canonical" href="https://www.collectandgo.be/nl/assortiment/boni-walnootpitten-200g" />
            <link rel="alternate" hreflang="fr-be" href="https://www.collectandgo.be/fr/assortiment/boni-cerneaux-de-noix-200g" />
            <div id="entitledItem_3074457345616678942" style="display:none;">"#;
        let p = parse_product_page(html).unwrap();
        assert_eq!(p.product_id, "3074457345616678942");
        assert_eq!(p.urls.len(), 2);
    }
}
