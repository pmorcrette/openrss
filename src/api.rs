//! JSON REST API used by the web UI.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::{
    AppState,
    db::{ArticleQuery, CategoryFilter, FeedUpdate, ReadStatus, Theme},
    error::{AppError, AppResult},
    opml,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(|| async { Json(json!({ "status": "ok" })) }))
        .route("/stats", get(stats))
        .route("/categories", get(list_categories).post(create_category))
        .route("/categories/{id}", axum::routing::patch(update_category).delete(delete_category))
        .route("/feeds", get(list_feeds).post(add_feed))
        .route("/feeds/{id}", get(get_feed).patch(update_feed).delete(delete_feed))
        .route("/feeds/{id}/refresh", post(refresh_feed))
        .route("/refresh", post(refresh_all))
        .route("/discover", post(discover))
        .route("/articles", get(list_articles))
        .route("/articles/mark", post(mark_articles))
        .route("/articles/mark-all-read", post(mark_all_read))
        .route("/articles/{id}", get(get_article).patch(update_article))
        .route("/opml", get(export_opml).post(import_opml))
        .route("/settings", get(get_settings).patch(update_settings))
        .route("/themes", get(list_themes).post(create_theme))
        .route("/themes/{id}", axum::routing::put(update_theme).delete(delete_theme))
        .fallback(|| async { AppError::not_found("Unknown API endpoint") })
}

/// Distinguishes an absent field from an explicit `null`.
fn double_option<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// Parses `?category=` values: an id, or `none` for uncategorized feeds.
fn category_filter(raw: Option<&str>) -> AppResult<Option<CategoryFilter>> {
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some("none") => Ok(Some(CategoryFilter::Uncategorized)),
        Some(s) => s
            .parse()
            .map(|id| Some(CategoryFilter::Id(id)))
            .map_err(|_| AppError::bad_request("category must be an id or \"none\"")),
    }
}

async fn stats(State(s): State<AppState>) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.store.stats()?))
}

// ---------- categories ----------

#[derive(Deserialize)]
struct CategoryBody {
    name: Option<String>,
    position: Option<i64>,
}

async fn list_categories(State(s): State<AppState>) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.store.list_categories()?))
}

async fn create_category(State(s): State<AppState>, Json(b): Json<CategoryBody>) -> AppResult<impl IntoResponse> {
    let cat = s.svc.store.create_category(b.name.as_deref().unwrap_or(""))?;
    Ok((StatusCode::CREATED, Json(cat)))
}

async fn update_category(State(s): State<AppState>, Path(id): Path<i64>, Json(b): Json<CategoryBody>) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.store.update_category(id, b.name.as_deref(), b.position)?))
}

async fn delete_category(State(s): State<AppState>, Path(id): Path<i64>) -> AppResult<impl IntoResponse> {
    s.svc.store.delete_category(id)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- feeds ----------

#[derive(Deserialize)]
struct FeedListQuery {
    category: Option<String>,
}

async fn list_feeds(State(s): State<AppState>, Query(q): Query<FeedListQuery>) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.store.list_feeds(category_filter(q.category.as_deref())?)?))
}

#[derive(Deserialize)]
struct AddFeedBody {
    url: String,
    category: Option<String>,
    title: Option<String>,
}

async fn add_feed(State(s): State<AppState>, Json(b): Json<AddFeedBody>) -> AppResult<impl IntoResponse> {
    let (feed, counts) = s.svc.add_feed(&b.url, b.category.as_deref(), b.title.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(json!({ "feed": feed, "articles_added": counts.added }))))
}

async fn get_feed(State(s): State<AppState>, Path(id): Path<i64>) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.store.get_feed(id)?))
}

#[derive(Deserialize)]
struct UpdateFeedBody {
    title: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    category_id: Option<Option<i64>>,
    /// Category by name, created if missing. Empty string removes the category.
    category: Option<String>,
    url: Option<String>,
}

async fn update_feed(State(s): State<AppState>, Path(id): Path<i64>, Json(b): Json<UpdateFeedBody>) -> AppResult<impl IntoResponse> {
    let category_id = match b.category {
        Some(name) => Some(s.svc.store.ensure_category(Some(&name))?),
        None => b.category_id,
    };
    let url = b.url.map(|u| crate::feeds::normalize_url(&u)).transpose()?;
    Ok(Json(s.svc.store.update_feed(id, FeedUpdate { title: b.title, category_id, url })?))
}

async fn delete_feed(State(s): State<AppState>, Path(id): Path<i64>) -> AppResult<impl IntoResponse> {
    s.svc.store.delete_feed(id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn refresh_feed(State(s): State<AppState>, Path(id): Path<i64>) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.refresh_feed(id).await?))
}

async fn refresh_all(State(s): State<AppState>) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.refresh_many(None).await?))
}

#[derive(Deserialize)]
struct DiscoverBody {
    url: String,
}

async fn discover(State(s): State<AppState>, Json(b): Json<DiscoverBody>) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.discover(&b.url).await?))
}

// ---------- articles ----------

#[derive(Deserialize)]
struct ArticleListQuery {
    feed_id: Option<i64>,
    category: Option<String>,
    #[serde(default)]
    status: ReadStatus,
    #[serde(default)]
    starred: bool,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    order: Option<String>,
}

async fn list_articles(State(s): State<AppState>, Query(q): Query<ArticleListQuery>) -> AppResult<impl IntoResponse> {
    let query = ArticleQuery {
        feed_id: q.feed_id,
        category: category_filter(q.category.as_deref())?,
        status: q.status,
        starred: q.starred,
        search: q.q.filter(|s| !s.trim().is_empty()),
        limit: q.limit.unwrap_or(50),
        offset: q.offset.unwrap_or(0),
        oldest_first: q.order.as_deref() == Some("oldest"),
    };
    Ok(Json(s.svc.store.list_articles(&query)?))
}

async fn get_article(State(s): State<AppState>, Path(id): Path<i64>) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.store.get_article(id)?))
}

#[derive(Deserialize)]
struct FlagsBody {
    #[serde(default)]
    ids: Vec<i64>,
    read: Option<bool>,
    starred: Option<bool>,
}

async fn update_article(State(s): State<AppState>, Path(id): Path<i64>, Json(b): Json<FlagsBody>) -> AppResult<impl IntoResponse> {
    if s.svc.store.set_article_flags(&[id], b.read, b.starred)? == 0 {
        return Err(AppError::not_found(format!("Article {id} not found")));
    }
    let a = s.svc.store.get_article(id)?;
    Ok(Json(json!({ "id": id, "is_read": a.meta.is_read, "is_starred": a.meta.is_starred })))
}

async fn mark_articles(State(s): State<AppState>, Json(b): Json<FlagsBody>) -> AppResult<impl IntoResponse> {
    let n = s.svc.store.set_article_flags(&b.ids, b.read, b.starred)?;
    Ok(Json(json!({ "updated": n })))
}

#[derive(Deserialize)]
struct MarkAllBody {
    feed_id: Option<i64>,
    category: Option<String>,
    before: Option<String>,
}

async fn mark_all_read(State(s): State<AppState>, body: Option<Json<MarkAllBody>>) -> AppResult<impl IntoResponse> {
    let b = body.map(|Json(b)| b).unwrap_or(MarkAllBody { feed_id: None, category: None, before: None });
    let n = s.svc.store.mark_all_read(b.feed_id, category_filter(b.category.as_deref())?, b.before.as_deref())?;
    Ok(Json(json!({ "updated": n })))
}

// ---------- OPML ----------

async fn export_opml(State(s): State<AppState>) -> AppResult<impl IntoResponse> {
    let xml = opml::export(&s.svc.store.list_feeds(None)?, &s.svc.store.list_categories()?);
    Ok((
        [
            (header::CONTENT_TYPE, "text/x-opml; charset=utf-8"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"openrss.opml\""),
        ],
        xml,
    ))
}

async fn import_opml(State(s): State<AppState>, body: String) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.import_opml(&body).await?))
}

// ---------- settings & themes ----------

async fn get_settings(State(s): State<AppState>) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.store.get_settings()?))
}

async fn update_settings(State(s): State<AppState>, Json(patch): Json<serde_json::Map<String, Value>>) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.store.set_settings(&patch)?))
}

async fn list_themes(State(s): State<AppState>) -> AppResult<impl IntoResponse> {
    Ok(Json(s.svc.store.list_themes()?))
}

async fn create_theme(State(s): State<AppState>, Json(mut t): Json<Theme>) -> AppResult<impl IntoResponse> {
    t.id = None;
    Ok((StatusCode::CREATED, Json(s.svc.store.save_theme(t)?)))
}

async fn update_theme(State(s): State<AppState>, Path(id): Path<i64>, Json(mut t): Json<Theme>) -> AppResult<impl IntoResponse> {
    t.id = Some(id);
    Ok(Json(s.svc.store.save_theme(t)?))
}

async fn delete_theme(State(s): State<AppState>, Path(id): Path<i64>) -> AppResult<impl IntoResponse> {
    s.svc.store.delete_theme(id)?;
    Ok(StatusCode::NO_CONTENT)
}
