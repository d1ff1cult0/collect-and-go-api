mod cogo;
mod config;
mod db;
mod scraper;
mod util;
mod web;

use std::sync::Arc;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

pub struct AppState {
    pub config: config::Config,
    pub db: db::Db,
    pub cogo: cogo::CogoClient,
    pub scrape_lock: scraper::ScrapeLock,
}

const USAGE: &str = "Gebruik: cogo-prices [serve|scrape|healthcheck]

  serve        (standaard) start de website + API en scrapet volgens SCRAPE_INTERVAL_HOURS
  scrape       voert één scrape uit en stopt
  healthcheck  controleert of de lokale server antwoordt (voor Docker)";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,tower_http=info")),
        )
        .init();

    let cmd = std::env::args().nth(1).unwrap_or_else(|| "serve".into());
    if !matches!(cmd.as_str(), "serve" | "scrape" | "healthcheck") {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    if cmd == "healthcheck" {
        return healthcheck().await;
    }

    let config = config::Config::from_env()?;
    let db = db::connect(&config.database_url).await?;
    let cogo = cogo::CogoClient::new(&config)?;
    let t = cogo.transport_status();
    tracing::info!(mode = t.mode, proxy = t.proxy, "verbinding met Collect&Go");
    let state = Arc::new(AppState {
        config,
        db,
        cogo,
        scrape_lock: Default::default(),
    });

    if cmd == "scrape" {
        match scraper::run_once(&state).await? {
            Some(s) => println!(
                "run {}: {} producten (van {}), {} nieuw, {} prijswijzigingen, {} URL-koppelingen",
                s.run_id,
                s.products,
                s.total.map(|t| t.to_string()).unwrap_or_else(|| "?".into()),
                s.stats.new_products,
                s.stats.prices_changed,
                s.urls_mapped
            ),
            None => println!("er loopt al een scrape"),
        }
        return Ok(());
    }

    if state.config.scheduler_enabled {
        tokio::spawn(scraper::scheduler(state.clone()));
    } else {
        tracing::info!("ingebouwde scheduler staat uit (SCHEDULER_ENABLED=false)");
    }

    let app = web::router(state.clone());
    let listener = tokio::net::TcpListener::bind(state.config.bind_addr).await?;
    tracing::info!(addr = %state.config.bind_addr, "server luistert");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn healthcheck() -> Result<()> {
    let addr: std::net::SocketAddr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;
    let port = addr.port();
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/healthz"))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?;
    anyhow::ensure!(resp.status().is_success(), "healthz gaf {}", resp.status());
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("afsluiten");
}
