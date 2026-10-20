pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod feeds;
pub mod opml;
pub mod web;

use std::{sync::Arc, time::Duration};

use axum::{Router, middleware};
use tower_http::{compression::CompressionLayer, trace::TraceLayer};

use crate::{config::Config, feeds::FeedService};

#[derive(Clone)]
pub struct AppState {
    pub svc: FeedService,
    pub config: Arc<Config>,
}

/// Builds the whole HTTP application: web UI and REST API.
pub fn app(state: AppState) -> Router {
    let router = Router::new()
        .nest("/api", api::router())
        .fallback(web::static_handler)
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_user))
        .layer(CompressionLayer::new());

    router.layer(TraceLayer::new_for_http()).with_state(state)
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
