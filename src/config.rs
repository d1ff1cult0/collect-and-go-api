use std::{net::SocketAddr, time::Duration};

use anyhow::{Context, Result};

/// De standaard X-CG-APIKey die de Collect&Go-website zelf meestuurt (`WCParamJS.apiGatewayKey`).
/// Wordt automatisch opnieuw opgehaald van de website als hij ooit niet meer werkt.
pub const DEFAULT_API_KEY: &str = "502b657c-624c-11eb-8024-f4d06b721e80";

#[derive(Clone, Debug)]
pub struct RelayConfig {
    pub url: String,
    pub secret: String,
    /// `true`: alles via de relay; `false`: enkel wanneer Collect&Go de rechtstreekse weg tegenhoudt.
    pub always: bool,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub database_url: String,
    /// Hoe vaak er gescraped wordt.
    pub scrape_interval: Duration,
    /// Scrape meteen bij het opstarten als de laatste geslaagde run ouder is dan het interval.
    pub scrape_on_startup: bool,
    /// Zet de ingebouwde scheduler volledig uit (bv. als je extern triggert).
    pub scheduler_enabled: bool,
    pub page_size: u32,
    /// Enkel voor testen: stop na zoveel pagina's (0 = geen limiet).
    pub max_pages: u32,
    /// Pauze tussen opeenvolgende requests naar Collect&Go.
    pub request_delay: Duration,
    pub api_key: String,
    pub user_agent: String,
    /// HTTP(S)/SOCKS5-proxy voor alle rechtstreekse aanvragen naar Collect&Go (`COGO_PROXY`).
    pub proxy: Option<String>,
    /// De relay (`relay/server.cjs`), voor wanneer Collect&Go het IP van de server weigert.
    pub relay: Option<RelayConfig>,
    /// Token voor `POST /api/v1/admin/scrape`. Zonder token is het endpoint uitgeschakeld.
    pub admin_token: Option<String>,
    /// Publieke basis-URL van deze dienst, enkel gebruikt in de API-documentatie.
    pub public_base_url: String,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind_addr = env_or("BIND_ADDR", "0.0.0.0:8080")
            .parse()
            .context("BIND_ADDR is geen geldig adres")?;
        let hours: f64 = env_or("SCRAPE_INTERVAL_HOURS", "6")
            .parse()
            .context("SCRAPE_INTERVAL_HOURS moet een getal zijn")?;
        anyhow::ensure!(hours > 0.0, "SCRAPE_INTERVAL_HOURS moet groter dan 0 zijn");
        let page_size: u32 = env_or("SCRAPE_PAGE_SIZE", "500")
            .parse()
            .context("SCRAPE_PAGE_SIZE moet een getal zijn")?;
        let delay_ms: u64 = env_or("SCRAPE_REQUEST_DELAY_MS", "1500")
            .parse()
            .context("SCRAPE_REQUEST_DELAY_MS moet een getal zijn")?;

        let relay = match std::env::var("COGO_RELAY_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
        {
            None => None,
            Some(url) => {
                let secret = std::env::var("COGO_RELAY_SECRET").unwrap_or_default();
                anyhow::ensure!(
                    secret.len() >= 16,
                    "COGO_RELAY_SECRET ontbreekt of is korter dan 16 tekens (zelfde waarde als RELAY_SECRET van de relay)"
                );
                let always = match env_or("COGO_RELAY_MODE", "fallback")
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "always" | "altijd" => true,
                    "fallback" => false,
                    other => anyhow::bail!(
                        "COGO_RELAY_MODE moet `fallback` of `always` zijn, niet `{other}`"
                    ),
                };
                Some(RelayConfig {
                    url: url.trim().to_string(),
                    secret,
                    always,
                })
            }
        };

        Ok(Self {
            bind_addr,
            database_url: env_or("DATABASE_URL", "sqlite://data/cogo.db"),
            scrape_interval: Duration::from_secs_f64(hours * 3600.0),
            scrape_on_startup: env_bool("SCRAPE_ON_STARTUP", true),
            scheduler_enabled: env_bool("SCHEDULER_ENABLED", true),
            page_size: page_size.clamp(24, 1000),
            max_pages: env_or("SCRAPE_MAX_PAGES", "0")
                .parse()
                .context("SCRAPE_MAX_PAGES moet een getal zijn")?,
            request_delay: Duration::from_millis(delay_ms),
            api_key: env_or("COGO_API_KEY", DEFAULT_API_KEY),
            user_agent: env_or(
                "COGO_USER_AGENT",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
            ),
            proxy: std::env::var("COGO_PROXY")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            relay,
            admin_token: std::env::var("ADMIN_TOKEN")
                .ok()
                .filter(|t| !t.trim().is_empty()),
            public_base_url: env_or("PUBLIC_BASE_URL", "http://localhost:8080")
                .trim_end_matches('/')
                .to_string(),
        })
    }
}
