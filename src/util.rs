use std::sync::LazyLock;

use regex::Regex;

pub fn xml_unescape(s: &str) -> String {
    s.replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Lowercase, accenten weg, enkel letters/cijfers met enkele spaties. Gebruikt voor zoeken.
pub fn search_normalize(s: &str) -> String {
    let ascii = deunicode::deunicode(s).to_lowercase();
    let mut out = String::with_capacity(ascii.len());
    let mut last_space = true;
    for c in ascii.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim_end().to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductPath {
    /// Genormaliseerd pad, bv. `/nl/assortiment/côte-dor-noir-|-orange-240g`.
    pub key: String,
    pub lang: &'static str,
    /// Pad zoals het naar collectandgo.be gestuurd kan worden (percent-encoded).
    pub request_path: String,
}

/// Een Collect&Go-link die naar één product wijst.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CogoLink {
    /// Productpagina: `/nl/assortiment/<slug>`.
    Page(ProductPath),
    /// Product dat in een zoek- of categorielijst is opengeklikt: `...#pdp_<id>`. Het getal is het
    /// ID van de SKU (het "item"), niet van het product zelf.
    Sku(String),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UrlError {
    #[error("dit is geen geldige URL")]
    Invalid,
    #[error("dit is geen collectandgo.be-URL")]
    WrongHost,
    #[error(
        "dit is geen link naar één Collect&Go-product (verwacht: https://www.collectandgo.be/nl/assortiment/<product>, of een zoeklink die eindigt op #pdp_<nummer>)"
    )]
    NotAProduct,
}

static PRODUCT_PATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)/(nl|fr)/(assortiment)/([^/]+)/?$").unwrap());

static PDP_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^pdp_(\d{1,20})$").unwrap());

/// Parset een URL (ook zonder schema) en controleert dat hij op collectandgo.be staat.
fn parse_cogo_url(raw: &str) -> Result<url::Url, UrlError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(UrlError::Invalid);
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let url = url::Url::parse(&with_scheme).map_err(|_| UrlError::Invalid)?;
    let host = url
        .host_str()
        .ok_or(UrlError::Invalid)?
        .to_ascii_lowercase();
    if host != "collectandgo.be" && !host.ends_with(".collectandgo.be") {
        return Err(UrlError::WrongHost);
    }
    Ok(url)
}

fn product_path(url: &url::Url) -> Option<ProductPath> {
    let decoded = percent_encoding::percent_decode_str(url.path())
        .decode_utf8_lossy()
        .to_string();
    let caps = PRODUCT_PATH_RE.captures(&decoded)?;
    let lang = if caps[1].eq_ignore_ascii_case("fr") {
        "fr"
    } else {
        "nl"
    };
    let slug = caps[3].trim().to_lowercase();
    if slug.is_empty() {
        return None;
    }
    let key = format!("/{lang}/assortiment/{slug}");
    let request_path = format!(
        "/{lang}/assortiment/{}",
        percent_encoding::utf8_percent_encode(&caps[3], percent_encoding::NON_ALPHANUMERIC)
            .to_string()
            // Koppeltekens laten we leesbaar; collectandgo.be gebruikt ze letterlijk.
            .replace("%2D", "-")
            .replace("%2E", ".")
            .replace("%5F", "_")
    );
    Some(ProductPath {
        key,
        lang,
        request_path,
    })
}

/// Herkent Collect&Go-productpagina-URL's en geeft een genormaliseerde sleutel terug.
/// Aanvaardt ook URL's zonder schema (`www.collectandgo.be/nl/assortiment/...`),
/// met querystring/fragment, en met of zonder percent-encoding.
pub fn parse_product_url(raw: &str) -> Result<ProductPath, UrlError> {
    product_path(&parse_cogo_url(raw)?).ok_or(UrlError::NotAProduct)
}

/// Herkent elke Collect&Go-link die naar één product wijst: een productpagina, of een zoek- of
/// categoriepagina waarop een product is opengeklikt (`/nl/zoek?searchTerm=...#pdp_<id>`).
///
/// Een `#pdp_<id>` gaat voor op het pad: een categoriepagina ziet er net zo uit als een
/// productpagina (`/nl/assortiment/suiker-pak#pdp_<id>`), maar het product is dat in het fragment.
pub fn parse_cogo_link(raw: &str) -> Result<CogoLink, UrlError> {
    let url = parse_cogo_url(raw)?;
    if let Some(c) = url.fragment().and_then(|f| PDP_RE.captures(f)) {
        return Ok(CogoLink::Sku(c[1].to_string()));
    }
    product_path(&url)
        .map(CogoLink::Page)
        .ok_or(UrlError::NotAProduct)
}

/// 319 -> "3,19"
pub fn fmt_cents(c: i64) -> String {
    let sign = if c < 0 { "-" } else { "" };
    let c = c.abs();
    format!("{sign}{},{:02}", c / 100, c % 100)
}

pub fn cents_to_f64(c: i64) -> f64 {
    c as f64 / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_search() {
        assert_eq!(
            search_normalize("CÔTE D'OR  Mignonnette"),
            "cote d or mignonnette"
        );
        assert_eq!(
            search_normalize("Plan't salami 100g!"),
            "plan t salami 100g"
        );
    }

    #[test]
    fn product_urls() {
        let a = parse_product_url(
            "https://www.collectandgo.be/nl/assortiment/c%C3%B4te-dor-noir-%7C-orange-240g",
        )
        .unwrap();
        let b = parse_product_url(
            "www.collectandgo.be/nl/assortiment/côte-dor-noir-|-orange-240g?x=1#top",
        )
        .unwrap();
        assert_eq!(a.key, b.key);
        assert_eq!(a.key, "/nl/assortiment/côte-dor-noir-|-orange-240g");
        assert_eq!(
            a.request_path,
            "/nl/assortiment/c%C3%B4te-dor-noir-%7C-orange-240g"
        );
        let fr =
            parse_product_url("https://collectandgo.be/FR/assortiment/Boni-Cerneaux-De-Noix-200g/")
                .unwrap();
        assert_eq!(fr.lang, "fr");
        assert_eq!(fr.key, "/fr/assortiment/boni-cerneaux-de-noix-200g");
        assert_eq!(
            parse_product_url("https://www.colruyt.be/nl/assortiment/x"),
            Err(UrlError::WrongHost)
        );
        assert_eq!(
            parse_product_url("https://www.collectandgo.be/nl/home"),
            Err(UrlError::NotAProduct)
        );
        assert_eq!(parse_product_url(""), Err(UrlError::Invalid));
        assert_eq!(
            parse_product_url("https://evilcollectandgo.be/nl/assortiment/x"),
            Err(UrlError::WrongHost)
        );
    }

    #[test]
    fn cogo_links() {
        let zoek = "https://www.collectandgo.be/nl/zoek?searchTerm=bier&filters=ads_f10007_ntk_cs%253A%2522Duvel%2522#pdp_3074457345616737905";
        assert_eq!(
            parse_cogo_link(zoek),
            Ok(CogoLink::Sku("3074457345616737905".into()))
        );
        assert_eq!(
            parse_cogo_link(
                "www.collectandgo.be/nl/zoek?searchTerm=aflegem#pdp_3074457345616848647"
            ),
            Ok(CogoLink::Sku("3074457345616848647".into()))
        );
        assert!(matches!(
            parse_cogo_link("https://www.collectandgo.be/nl/assortiment/boni-walnootpitten-200g"),
            Ok(CogoLink::Page(p)) if p.key == "/nl/assortiment/boni-walnootpitten-200g"
        ));
        // Categoriepagina met een opengeklikt product: het fragment telt, niet het pad.
        assert_eq!(
            parse_cogo_link(
                "https://www.collectandgo.be/nl/assortiment/suiker-alternatief?rootCategoryId=20011#pdp_3074457345616687717"
            ),
            Ok(CogoLink::Sku("3074457345616687717".into()))
        );
        assert_eq!(
            parse_cogo_link(
                "https://www.collectandgo.be/colruyt/nl/assortiment/suiker-pak#pdp_3074457345616685309"
            ),
            Ok(CogoLink::Sku("3074457345616685309".into()))
        );
        // Zoekpagina zonder opengeklikt product, of een fragment dat geen product is.
        assert_eq!(
            parse_cogo_link("https://www.collectandgo.be/nl/zoek?searchTerm=bier"),
            Err(UrlError::NotAProduct)
        );
        assert_eq!(
            parse_cogo_link("https://www.collectandgo.be/nl/zoek?searchTerm=bier#pdp_abc"),
            Err(UrlError::NotAProduct)
        );
        assert_eq!(
            parse_cogo_link("https://www.colruyt.be/nl/zoek#pdp_3074457345616848647"),
            Err(UrlError::WrongHost)
        );
        // Voor de sitemap blijft enkel een productpagina geldig.
        assert_eq!(parse_product_url(zoek), Err(UrlError::NotAProduct));
    }

    #[test]
    fn money() {
        assert_eq!(fmt_cents(319), "3,19");
        assert_eq!(fmt_cents(5), "0,05");
        assert_eq!(fmt_cents(-120), "-1,20");
    }
}
