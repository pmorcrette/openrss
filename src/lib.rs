pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod feeds;
pub mod mcp;
pub mod opml;
pub mod web;

use std::{sync::Arc, time::Duration};

use axum::{Router, middleware};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tower_http::{
    compression::CompressionLayer,
    trace::{DefaultOnFailure, TraceLayer},
};

use crate::{config::Config, feeds::FeedService};

#[derive(Clone)]
pub struct AppState {
    pub svc: FeedService,
    pub config: Arc<Config>,
}

/// Builds the whole HTTP application: web UI, REST API and MCP endpoint.
pub fn app(state: AppState) -> Router {
    let mut router = Router::new()
        .nest("/api", api::router())
        .fallback(web::static_handler)
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_user))
        .layer(CompressionLayer::new());

    if state.config.mcp_enabled {
        let svc = state.svc.clone();
        let mut mcp_config = StreamableHttpServerConfig::default();
        // Stateless mode: every request is self-contained, so restarts never break agents.
        mcp_config.legacy_session_mode = false;
        mcp_config.json_response = true;
        mcp_config = if state.config.mcp_allowed_hosts.is_empty() {
            mcp_config.disable_allowed_hosts()
        } else {
            mcp_config.with_allowed_hosts(state.config.mcp_allowed_hosts.clone())
        };
        let mcp = StreamableHttpService::<_, LocalSessionManager>::new(
            move || Ok(mcp::OpenRssMcp::new(svc.clone())),
            Default::default(),
            mcp_config,
        );
        let mcp_router = Router::new()
            .nest_service("/mcp", mcp)
            .layer(middleware::from_fn_with_state(state.clone(), auth::require_agent));
        router = mcp_router.merge(router);
    }

    // Upstream feed failures surface as 502s; they are not server errors worth logging loudly.
    let trace = TraceLayer::new_for_http().on_failure(DefaultOnFailure::new().level(tracing::Level::DEBUG));
    router.layer(trace).with_state(state)
}

/// Periodically refreshes every feed and prunes old articles.
pub async fn run_scheduler(state: AppState) {
    tokio::time::sleep(Duration::from_secs(5)).await;
    loop {
        match state.svc.refresh_many(None).await {
            Ok(s) => tracing::info!(feeds = s.feeds, added = s.added, errors = s.errors, "refreshed feeds"),
            Err(e) => tracing::warn!("refresh failed: {e}"),
        }
        match state.svc.store.prune_articles(state.config.retention_days) {
            Ok(0) => {}
            Ok(n) => tracing::info!("pruned {n} old articles"),
            Err(e) => tracing::warn!("pruning failed: {e}"),
        }
        tokio::time::sleep(state.config.refresh_interval).await;
    }
}
