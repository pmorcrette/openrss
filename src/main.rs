use std::sync::Arc;

use anyhow::Context;
use openrss::{AppState, config::Config, db::Store, feeds::FeedService, mcp::OpenRssMcp};
use rmcp::ServiceExt;
use tracing_subscriber::EnvFilter;

const HELP: &str = "OpenRSS - a self-hosted RSS reader with an MCP server

Usage:
  openrss [serve]      Start the web UI, REST API and MCP endpoint (/mcp)
  openrss mcp          Run the MCP server over stdio (for local agents)
  openrss healthcheck  Exit 0 if the local server answers /api/health

Configuration is read from environment variables; see README.md.";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let command = std::env::args().nth(1).unwrap_or_else(|| "serve".into());
    let stdio = command == "mcp";
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_env("LOG_LEVEL").unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn")))
        // stdout carries the MCP protocol in stdio mode, so log to stderr.
        .with_writer(std::io::stderr)
        .with_ansi(!stdio)
        .init();

    let config = Config::from_env();
    match command.as_str() {
        "serve" => serve(config).await,
        "mcp" => mcp_stdio(config).await,
        "healthcheck" => healthcheck(config).await,
        "-h" | "--help" | "help" => {
            println!("{HELP}");
            Ok(())
        }
        "-V" | "--version" => {
            println!("openrss {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => anyhow::bail!("unknown command {other:?}\n\n{HELP}"),
    }
}

fn open_service(config: &Config) -> anyhow::Result<FeedService> {
    let store = Store::open(&config.db_path()).with_context(|| format!("opening database {}", config.db_path().display()))?;
    FeedService::new(store, config)
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let state = AppState { svc: open_service(&config)?, config: Arc::new(config) };
    let c = &state.config;
    if c.auth_password.is_none() {
        tracing::warn!("AUTH_PASSWORD is not set: the web UI and REST API are open to anyone who can reach them");
    }
    if c.mcp_enabled && c.api_token.is_none() && c.auth_password.is_none() {
        tracing::warn!("API_TOKEN is not set: the MCP endpoint is unauthenticated");
    }
    tokio::spawn(openrss::run_scheduler(state.clone()));

    let listener = tokio::net::TcpListener::bind(&c.bind).await.with_context(|| format!("binding {}", c.bind))?;
    tracing::info!("OpenRSS listening on http://{}", listener.local_addr()?);
    if c.mcp_enabled {
        tracing::info!("MCP endpoint available at /mcp");
    }
    axum::serve(listener, openrss::app(state.clone()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Resolves on Ctrl-C or SIGTERM (what `docker stop` sends).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    tracing::info!("shutting down");
}

async fn mcp_stdio(config: Config) -> anyhow::Result<()> {
    let svc = open_service(&config)?;
    let server = OpenRssMcp::new(svc).serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}

async fn healthcheck(config: Config) -> anyhow::Result<()> {
    let port = config.bind.rsplit(':').next().unwrap_or("8080");
    let res = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/api/health"))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?;
    anyhow::ensure!(res.status().is_success(), "unhealthy: HTTP {}", res.status());
    Ok(())
}
