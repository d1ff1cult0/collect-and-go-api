//! JSON-representaties die zowel de API als de pagina's gebruiken.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::{
    cogo,
    db::{PriceRow, ProductWithPrice},
    util::cents_to_f64,
};

#[derive(Debug, Serialize)]
pub struct ProductJson {
    pub id: String,
    pub article_number: String,
    pub name: String,
    pub long_name: Option<String>,
    pub brand: Option<String>,
    pub content: Option<String>,
    pub image_url: Option<String>,
    /// Link naar het product op collectandgo.be (indien gekend).
    pub url: Option<String>,
    pub is_available: bool,
    pub promotions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct OfferJson {
    /// Stukprijs bij aankoop van minstens `min_quantity` stuks.
    pub price: f64,
    pub min_quantity: Option<i64>,
    pub unit_price: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct PriceJson {
    pub currency: &'static str,
    /// Normale prijs per stuk in euro.
    pub price: f64,
    /// Prijs per eenheid (`unit`), bv. per kg of per liter.
    pub unit_price: Option<f64>,
    pub unit: Option<String>,
    /// Hoeveelheidskorting ("vanaf 3 stuks").
    pub offer: Option<OfferJson>,
    /// Referentieprijs die Collect&Go toont bij sommige rode prijzen.
    pub reference_price: Option<f64>,
    pub is_red_price: bool,
}

#[derive(Debug, Serialize)]
pub struct ProductPriceJson {
    pub product: ProductJson,
    pub price: Option<PriceJson>,
    /// Laatste keer dat we deze prijs op Collect&Go hebben waargenomen.
    pub last_updated_at: Option<DateTime<Utc>>,
    /// Moment waarop de huidige prijs voor het eerst werd waargenomen (= laatste prijswijziging).
    /// Is gelijk aan `tracked_since` als de prijs sinds het begin van de opvolging niet wijzigde.
    pub last_changed_at: Option<DateTime<Utc>>,
    /// De prijs vóór de laatste wijziging.
    pub previous_price: Option<f64>,
    /// Sinds wanneer dit product opgevolgd wordt.
    pub tracked_since: DateTime<Utc>,
    pub history_url: String,
    pub page_url: String,
}

pub fn image_url(path: Option<&str>) -> Option<String> {
    path.filter(|p| !p.is_empty())
        .map(|p| format!("{}{}", cogo::IMAGE_BASE, p.trim_start_matches('/')))
}

pub fn site_url(path: Option<&str>) -> Option<String> {
    path.map(|p| {
        let encoded: String = p
            .split('/')
            .map(|seg| {
                percent_encoding::utf8_percent_encode(seg, percent_encoding::NON_ALPHANUMERIC)
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("/")
            .replace("%2D", "-")
            .replace("%2E", ".")
            .replace("%5F", "_");
        format!("{}{encoded}", cogo::SITE)
    })
}

pub fn promotions(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

impl ProductPriceJson {
    pub fn from_row(p: &ProductWithPrice, base_url: &str) -> Self {
        let r = &p.product;
        let price = p.base_price.map(|base| PriceJson {
            currency: "EUR",
            price: cents_to_f64(base),
            unit_price: p.unit_price.map(cents_to_f64),
            unit: r.unit.clone(),
            offer: p.offer_price.map(|o| OfferJson {
                price: cents_to_f64(o),
                min_quantity: p.offer_quantity,
                unit_price: p.offer_unit_price.map(cents_to_f64),
            }),
            reference_price: p.reference_price.map(cents_to_f64),
            is_red_price: p.is_red_price.unwrap_or(false),
        });
        Self {
            product: ProductJson {
                id: r.id.clone(),
                article_number: r.part_number.trim_end_matches("-P").to_string(),
                name: r.name.clone(),
                long_name: r.long_name.clone(),
                brand: r.brand.clone(),
                content: r.content.clone(),
                image_url: image_url(r.image_path.as_deref()),
                url: site_url(p.url_path.as_deref()),
                is_available: r.is_available,
                promotions: promotions(&r.promotions),
            },
            price,
            last_updated_at: p.price_last_seen_at,
            last_changed_at: p.price_first_seen_at,
            previous_price: p.previous_price.map(cents_to_f64),
            tracked_since: r.first_seen_at,
            history_url: format!("{base_url}/api/v1/products/{}/history", r.id),
            page_url: format!("{base_url}/product/{}", r.id),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct HistoryPointJson {
    pub price: f64,
    pub unit_price: Option<f64>,
    pub offer_price: Option<f64>,
    pub offer_min_quantity: Option<i64>,
    pub reference_price: Option<f64>,
    pub is_red_price: bool,
    /// Eerste waarneming van deze prijs.
    pub from: DateTime<Utc>,
    /// Laatste waarneming van deze prijs.
    pub until: DateTime<Utc>,
}

impl From<&PriceRow> for HistoryPointJson {
    fn from(r: &PriceRow) -> Self {
        Self {
            price: cents_to_f64(r.base_price),
            unit_price: r.unit_price.map(cents_to_f64),
            offer_price: r.offer_price.map(cents_to_f64),
            offer_min_quantity: r.offer_quantity,
            reference_price: r.reference_price.map(cents_to_f64),
            is_red_price: r.is_red_price,
            from: r.first_seen_at,
            until: r.last_seen_at,
        }
    }
}
