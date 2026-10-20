//! Fetching, parsing, sanitizing and discovering feeds.

use std::{collections::HashSet, sync::Arc, time::Duration};

use futures::{StreamExt, stream};
use reqwest::{StatusCode, header};
use scraper::{Html, Selector};
use serde::Serialize;
use tokio::sync::Mutex;
use url::Url;

use crate::{
    config::Config,
    db::{Feed, FeedMeta, NewArticle, Store, UpsertCounts, format_time},
    error::{AppError, AppResult},
};

const MAX_BODY_BYTES: usize = 15 * 1024 * 1024;
const SUMMARY_CHARS: usize = 320;

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredFeed {
    pub url: String,
    pub title: Option<String>,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefreshResult {
    pub feed_id: i64,
    pub title: String,
    pub added: usize,
    pub updated: usize,
    pub not_modified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefreshSummary {
    pub feeds: usize,
    pub added: usize,
    pub updated: usize,
    pub errors: usize,
    pub results: Vec<RefreshResult>,
}

struct Fetched {
    body: Vec<u8>,
    final_url: String,
    etag: Option<String>,
    last_modified: Option<String>,
}

enum FetchOutcome {
    NotModified,
    Body(Fetched),
}

/// Shared service that owns the HTTP client and talks to the store.
#[derive(Clone)]
pub struct FeedService {
    pub store: Store,
    client: reqwest::Client,
    concurrency: usize,
    refresh_lock: Arc<Mutex<()>>,
}

impl FeedService {
    pub fn new(store: Store, config: &Config) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(&config.user_agent)
            .timeout(config.fetch_timeout)
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(8))
            .build()?;
        Ok(Self { store, client, concurrency: config.fetch_concurrency, refresh_lock: Arc::new(Mutex::new(())) })
    }

    async fn fetch(&self, url: &str, etag: Option<&str>, last_modified: Option<&str>) -> AppResult<FetchOutcome> {
        let mut req = self.client.get(url).header(
            header::ACCEPT,
            "application/rss+xml, application/atom+xml, application/feed+json, application/json;q=0.9, application/xml;q=0.9, text/xml;q=0.9, text/html;q=0.8, */*;q=0.5",
        );
        if let Some(e) = etag {
            req = req.header(header::IF_NONE_MATCH, e);
        }
        if let Some(lm) = last_modified {
            req = req.header(header::IF_MODIFIED_SINCE, lm);
        }
        let mut resp = req.send().await.map_err(|e| AppError::bad_gateway(describe_reqwest_error(&e)))?;
        if resp.status() == StatusCode::NOT_MODIFIED {
            return Ok(FetchOutcome::NotModified);
        }
        if !resp.status().is_success() {
            return Err(AppError::bad_gateway(format!("HTTP {} from {url}", resp.status())));
        }
        let headers = resp.headers().clone();
        let get = |h: header::HeaderName| headers.get(h).and_then(|v| v.to_str().ok()).map(str::to_string);
        let final_url = resp.url().to_string();
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| AppError::bad_gateway(describe_reqwest_error(&e)))? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_BODY_BYTES {
                return Err(AppError::bad_gateway("Response is too large"));
            }
        }
        Ok(FetchOutcome::Body(Fetched {
            body,
            final_url,
            etag: get(header::ETAG),
            last_modified: get(header::LAST_MODIFIED),
        }))
    }

    /// Finds feeds for a URL: the URL itself if it is a feed, the feeds an HTML
    /// page advertises, or feeds at common locations.
    pub async fn discover(&self, input: &str) -> AppResult<Vec<DiscoveredFeed>> {
        let url = normalize_url(input)?;
        let FetchOutcome::Body(page) = self.fetch(&url, None, None).await? else {
            return Err(AppError::bad_gateway("Unexpected 304 response"));
        };
        if let Ok((meta, _)) = parse_feed(&page.body, &page.final_url) {
            return Ok(vec![DiscoveredFeed { url: page.final_url, title: non_empty(meta.title), kind: "feed".into() }]);
        }
        let mut found = discover_in_html(&String::from_utf8_lossy(&page.body), &page.final_url);
        if found.is_empty() {
            let base = Url::parse(&page.final_url).map_err(|e| AppError::bad_request(e.to_string()))?;
            for path in ["/feed", "/rss", "/feed.xml", "/rss.xml", "/atom.xml", "/index.xml", "/feed.json", "/rss/"] {
                let Ok(candidate) = base.join(path) else { continue };
                if let Ok(FetchOutcome::Body(f)) = self.fetch(candidate.as_str(), None, None).await {
                    if let Ok((meta, _)) = parse_feed(&f.body, &f.final_url) {
                        found.push(DiscoveredFeed { url: f.final_url, title: non_empty(meta.title), kind: "feed".into() });
                        break;
                    }
                }
            }
        }
        if found.is_empty() {
            return Err(AppError::not_found(format!("No RSS, Atom or JSON feed found at {url}")));
        }
        Ok(found)
    }

    /// Subscribes to a feed (with auto-discovery) and imports its current entries.
    pub async fn add_feed(&self, input: &str, category: Option<&str>, title: Option<&str>) -> AppResult<(Feed, UpsertCounts)> {
        let url = normalize_url(input)?;
        if let Some(existing) = self.store.find_feed_by_url(&url)? {
            return Err(AppError::conflict(format!("Already subscribed to {} (feed {})", existing.url, existing.id)));
        }
        let FetchOutcome::Body(mut fetched) = self.fetch(&url, None, None).await? else {
            return Err(AppError::bad_gateway("Unexpected 304 response"));
        };
        let parsed = match parse_feed(&fetched.body, &fetched.final_url) {
            Ok(p) => p,
            Err(parse_err) if looks_like_feed_document(&fetched.body) => return Err(parse_err),
            Err(parse_err) => {
                // Not a feed: treat it as a web page and follow its advertised feed.
                let html = String::from_utf8_lossy(&fetched.body).into_owned();
                let Some(found) = discover_in_html(&html, &fetched.final_url).into_iter().next() else {
                    let discovered = self.discover(&url).await.map_err(|_| parse_err)?;
                    return Box::pin(self.add_feed(&discovered[0].url, category, title)).await;
                };
                let FetchOutcome::Body(f) = self.fetch(&found.url, None, None).await? else {
                    return Err(AppError::bad_gateway("Unexpected 304 response"));
                };
                fetched = f;
                parse_feed(&fetched.body, &fetched.final_url)?
            }
        };
        let feed_url = fetched.final_url.clone();
        if let Some(existing) = self.store.find_feed_by_url(&feed_url)? {
            return Err(AppError::conflict(format!("Already subscribed to {} (feed {})", existing.url, existing.id)));
        }
        let (mut meta, items) = parsed;
        meta.etag = fetched.etag;
        meta.last_modified = fetched.last_modified;
        let category_id = self.store.ensure_category(category)?;
        let id = self.store.insert_feed(&feed_url, &meta, category_id, title)?;
        let counts = self.store.upsert_articles(id, &items)?;
        Ok((self.store.get_feed(id)?, counts))
    }

    pub async fn refresh_feed(&self, id: i64) -> AppResult<RefreshResult> {
        let (url, etag, last_modified) = self.store.feed_fetch_state(id)?;
        let title = self.store.get_feed(id)?.title;
        let mut result = RefreshResult { feed_id: id, title, added: 0, updated: 0, not_modified: false, error: None };
        let outcome = async {
            match self.fetch(&url, etag.as_deref(), last_modified.as_deref()).await? {
                FetchOutcome::NotModified => {
                    self.store.record_fetch_not_modified(id)?;
                    Ok::<_, AppError>(None)
                }
                FetchOutcome::Body(f) => {
                    let (mut meta, items) = parse_feed(&f.body, &f.final_url)?;
                    meta.etag = f.etag;
                    meta.last_modified = f.last_modified;
                    let counts = self.store.upsert_articles(id, &items)?;
                    self.store.record_fetch_success(id, &meta)?;
                    Ok(Some(counts))
                }
            }
        }
        .await;
        match outcome {
            Ok(None) => result.not_modified = true,
            Ok(Some(c)) => {
                result.added = c.added;
                result.updated = c.updated;
            }
            Err(e) => {
                self.store.record_fetch_error(id, &e.message)?;
                result.error = Some(e.message);
            }
        }
        Ok(result)
    }

    /// Refreshes the given feeds (all when `None`) with bounded concurrency.
    pub async fn refresh_many(&self, ids: Option<Vec<i64>>) -> AppResult<RefreshSummary> {
        let _guard = self.refresh_lock.lock().await;
        let ids = match ids {
            Some(ids) => ids,
            None => self.store.feed_ids()?,
        };
        let results: Vec<RefreshResult> = stream::iter(ids)
            .map(|id| async move { self.refresh_feed(id).await })
            .buffer_unordered(self.concurrency)
            .filter_map(|r| async move { r.ok() })
            .collect()
            .await;
        Ok(RefreshSummary {
            feeds: results.len(),
            added: results.iter().map(|r| r.added).sum(),
            updated: results.iter().map(|r| r.updated).sum(),
            errors: results.iter().filter(|r| r.error.is_some()).count(),
            results,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportFailure {
    pub url: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportSummary {
    pub found: usize,
    pub imported: usize,
    pub already_subscribed: usize,
    pub failed: Vec<ImportFailure>,
}

impl FeedService {
    /// Subscribes to every feed in an OPML document, keeping its folders.
    pub async fn import_opml(&self, xml: &str) -> AppResult<ImportSummary> {
        let entries = crate::opml::parse(xml)?;
        let found = entries.len();
        let mut already = 0;
        let mut todo = Vec::new();
        for e in entries {
            match normalize_url(&e.url) {
                Ok(u) if self.store.find_feed_by_url(&u)?.is_some() => already += 1,
                _ => todo.push(e),
            }
        }
        let outcomes: Vec<(String, AppResult<()>)> = stream::iter(todo)
            .map(|e| async move {
                let r = async {
                    let (feed, _) = self.add_feed(&e.url, e.category.as_deref(), None).await?;
                    // Keep a renamed subscription's name from the OPML file.
                    if let Some(t) = e.title.filter(|t| *t != feed.title) {
                        self.store.update_feed(feed.id, crate::db::FeedUpdate { title: Some(t), ..Default::default() })?;
                    }
                    Ok(())
                }
                .await;
                (e.url, r)
            })
            .buffer_unordered(self.concurrency)
            .collect()
            .await;
        let mut summary = ImportSummary { found, imported: 0, already_subscribed: already, failed: Vec::new() };
        for (url, r) in outcomes {
            match r {
                Ok(()) => summary.imported += 1,
                Err(e) if e.status == reqwest::StatusCode::CONFLICT => summary.already_subscribed += 1,
                Err(e) => summary.failed.push(ImportFailure { url, error: e.message }),
            }
        }
        Ok(summary)
    }
}

fn describe_reqwest_error(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        msg = format!("{msg}: {s}");
        source = s.source();
    }
    msg
}

fn non_empty(s: String) -> Option<String> {
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Accepts bare domains ("example.com") and `feed://` URLs.
pub fn normalize_url(input: &str) -> AppResult<String> {
    let s = input.trim();
    if s.is_empty() {
        return Err(AppError::bad_request("URL is required"));
    }
    let s = s.strip_prefix("feed:").map(|r| r.trim_start_matches("//")).unwrap_or(s);
    let with_scheme = if s.contains("://") { s.to_string() } else { format!("https://{s}") };
    let url = Url::parse(&with_scheme).map_err(|e| AppError::bad_request(format!("Invalid URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::bad_request("Only http and https URLs are supported"));
    }
    Ok(url.to_string())
}

/// Collects `<link rel="alternate">` feeds advertised by an HTML page.
pub fn discover_in_html(html: &str, base: &str) -> Vec<DiscoveredFeed> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("link[href]").expect("valid selector");
    let base = Url::parse(base).ok();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for el in doc.select(&sel) {
        let rel = el.value().attr("rel").unwrap_or("").to_lowercase();
        if !rel.split_whitespace().any(|r| r == "alternate") {
            continue;
        }
        let ty = el.value().attr("type").unwrap_or("").to_lowercase();
        let kind = if ty.contains("rss") {
            "rss"
        } else if ty.contains("atom") {
            "atom"
        } else if ty.contains("feed+json") || ty.contains("json") {
            "json"
        } else {
            continue;
        };
        let href = el.value().attr("href").unwrap_or("");
        let Some(url) = base.as_ref().and_then(|b| b.join(href).ok()).or_else(|| Url::parse(href).ok()) else {
            continue;
        };
        // Skip per-post comment feeds when the page also has a main feed.
        if seen.insert(url.to_string()) {
            out.push(DiscoveredFeed {
                url: url.to_string(),
                title: el.value().attr("title").map(str::to_string).and_then(non_empty),
                kind: kind.into(),
            });
        }
    }
    out.sort_by_key(|f| f.url.contains("comment"));
    out
}

/// Parses RSS 0.9x/1.0/2.0, Atom or JSON Feed into normalized, sanitized entries.
pub fn parse_feed(body: &[u8], url: &str) -> AppResult<(FeedMeta, Vec<NewArticle>)> {
    let parser = feed_rs::parser::Builder::new().base_uri(Some(url)).build();
    let feed = match parser.parse(body) {
        Ok(f) => f,
        // Many real-world feeds contain unescaped ampersands; repair and retry once.
        Err(e) => match escape_bare_ampersands(body) {
            Some(fixed) => parser.parse(fixed.as_bytes()),
            None => Err(e),
        }
        .map_err(|e| AppError::bad_request(format!("Not a valid feed: {e}")))?,
    };

    let site_url = feed
        .links
        .iter()
        .find(|l| l.rel.as_deref().is_none_or(|r| r == "alternate") && !looks_like_feed_link(l))
        .or_else(|| feed.links.iter().find(|l| !looks_like_feed_link(l)))
        .map(|l| l.href.clone());
    let meta = FeedMeta {
        title: feed.title.as_ref().map(|t| plain_text(&t.content)).unwrap_or_default(),
        site_url: site_url.clone(),
        description: feed.description.as_ref().map(|t| plain_text(&t.content)).and_then(non_empty),
        ..Default::default()
    };
    let feed_base = site_url.as_deref().unwrap_or(url);

    let items = feed
        .entries
        .into_iter()
        .map(|e| {
            let link = e
                .links
                .iter()
                .find(|l| l.rel.as_deref().is_none_or(|r| r == "alternate"))
                .or(e.links.first())
                .map(|l| l.href.clone());
            let base = e.base.clone().or_else(|| link.clone()).unwrap_or_else(|| feed_base.to_string());
            let raw_html = e
                .content
                .as_ref()
                .and_then(|c| c.body.clone())
                .or_else(|| e.summary.as_ref().map(|s| text_as_html(s)))
                .or_else(|| e.media.iter().find_map(|m| m.description.as_ref().map(text_as_html)))
                .unwrap_or_default();
            let mut content = sanitize_html(&raw_html, &base);
            let enclosure = media_player(&e.media);
            if let Some(player) = &enclosure {
                content = format!("{player}{content}");
            }
            let text = plain_text(&content);
            let summary = match e.summary.as_ref().map(|s| plain_text(&s.content)).and_then(non_empty) {
                Some(s) if s.chars().count() > 20 => truncate(&s, SUMMARY_CHARS),
                _ => truncate(&text, SUMMARY_CHARS),
            };
            let title = e
                .title
                .as_ref()
                .map(|t| plain_text(&t.content))
                .and_then(non_empty)
                .unwrap_or_else(|| truncate(&text, 80));
            let image_url = e
                .media
                .iter()
                .flat_map(|m| m.thumbnails.iter().map(|t| t.image.uri.clone()))
                .next()
                .or_else(|| {
                    e.media.iter().flat_map(|m| &m.content).find_map(|c| {
                        let is_image = c.content_type.as_ref().is_some_and(|t| t.to_string().starts_with("image/"));
                        is_image.then(|| c.url.as_ref().map(|u| u.to_string())).flatten()
                    })
                })
                .or_else(|| first_image(&content));
            let author = e.authors.iter().filter_map(|a| a.name.clone()).map(|n| plain_text(&n)).find(|n| !n.is_empty());
            let published = e.published.or(e.updated).map(|d| d.min(chrono::Utc::now())).map(format_time);
            NewArticle {
                guid: if e.id.is_empty() { link.clone().unwrap_or_else(|| title.clone()) } else { e.id },
                url: link,
                title,
                author,
                summary,
                content,
                text,
                image_url,
                published_at: published,
            }
        })
        .collect();
    Ok((meta, items))
}

/// Replaces `&` that does not start an entity with `&amp;`. Returns `None` if nothing changed.
fn escape_bare_ampersands(body: &[u8]) -> Option<String> {
    static BARE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"&([^&;\s]{0,12})(;?)").unwrap());
    let text = std::str::from_utf8(body).ok()?;
    let mut changed = false;
    let fixed = BARE.replace_all(text, |c: &regex::Captures| {
        let name = &c[1];
        let is_entity = &c[2] == ";"
            && (name.starts_with('#') || (!name.is_empty() && name.chars().all(|ch| ch.is_ascii_alphanumeric())));
        if is_entity {
            c[0].to_string()
        } else {
            changed = true;
            format!("&amp;{name}{}", &c[2])
        }
    });
    changed.then(|| fixed.into_owned())
}

/// True when a response is plainly meant to be a feed (so a parse failure is the real error).
fn looks_like_feed_document(body: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&body[..body.len().min(1024)]).to_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    head.contains("<rss") || head.contains("<feed") || head.contains("<rdf:rdf") || head.contains("jsonfeed.org")
}

fn looks_like_feed_link(l: &feed_rs::model::Link) -> bool {
    l.rel.as_deref() == Some("self")
        || l.media_type.as_deref().is_some_and(|t| t.contains("rss") || t.contains("atom") || t.contains("xml"))
}

fn text_as_html(t: &feed_rs::model::Text) -> String {
    if t.content_type.to_string().contains("html") {
        t.content.clone()
    } else {
        // Plain text: preserve paragraphs.
        t.content
            .split("\n\n")
            .map(|p| format!("<p>{}</p>", html_escape(p.trim())))
            .collect::<Vec<_>>()
            .join("")
    }
}

fn media_player(media: &[feed_rs::model::MediaObject]) -> Option<String> {
    media.iter().flat_map(|m| &m.content).find_map(|c| {
        let ty = c.content_type.as_ref()?.to_string();
        let url = c.url.as_ref()?.to_string();
        let tag = if ty.starts_with("audio/") {
            "audio"
        } else if ty.starts_with("video/") {
            "video"
        } else {
            return None;
        };
        Some(format!(
            r#"<{tag} controls preload="none" src="{}"></{tag}>"#,
            html_escape(&url)
        ))
    })
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Removes scripts, styles, event handlers and other unsafe markup, makes
/// relative URLs absolute, and opens links in a new tab.
pub fn sanitize_html(html: &str, base: &str) -> String {
    let mut b = ammonia::Builder::default();
    b.add_tags([
        "figure", "figcaption", "picture", "source", "audio", "video", "details", "summary", "mark", "time", "section",
        "article", "aside", "header", "footer", "main", "nav",
    ])
    .add_tag_attributes("img", ["src", "srcset", "sizes", "alt", "title", "width", "height"])
    .add_tag_attributes("source", ["src", "srcset", "type", "media", "sizes"])
    .add_tag_attributes("audio", ["src", "controls", "preload"])
    .add_tag_attributes("video", ["src", "controls", "preload", "poster", "width", "height"])
    .add_tag_attributes("time", ["datetime"])
    .add_tag_attributes("a", ["title"])
    .rm_tags(["map", "area"])
    .link_rel(Some("noopener noreferrer nofollow"))
    .set_tag_attribute_value("a", "target", "_blank")
    .set_tag_attribute_value("img", "loading", "lazy")
    .set_tag_attribute_value("img", "referrerpolicy", "no-referrer")
    .strip_comments(true);
    if let Ok(u) = Url::parse(base) {
        b.url_relative(ammonia::UrlRelative::RewriteWithBase(u));
    } else {
        b.url_relative(ammonia::UrlRelative::Deny);
    }
    b.clean(html).to_string()
}

/// Extracts readable text from HTML, collapsing whitespace.
pub fn plain_text(html: &str) -> String {
    if !html.contains('<') && !html.contains('&') {
        return html.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    // Separate block elements with a space, but keep inline text (e.g. "<b>a</b>.") joined.
    static BLOCK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)<(/?(p|div|br|li|ul|ol|h[1-6]|tr|td|th|blockquote|pre|figure|figcaption|section|article|hr)\b)").unwrap()
    });
    let spaced = BLOCK.replace_all(html, " <$1");
    let frag = Html::parse_fragment(&spaced);
    let out: String = frag.root_element().text().collect();
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map(|(a, _)| a).unwrap_or(&cut);
    format!("{}…", cut.trim_end_matches(|c: char| c.is_ascii_punctuation()))
}

fn first_image(html: &str) -> Option<String> {
    let frag = Html::parse_fragment(html);
    let sel = Selector::parse("img[src]").expect("valid selector");
    frag.select(&sel)
        .filter(|img| {
            let tiny = |a: &str| img.value().attr(a).and_then(|v| v.parse::<u32>().ok()).is_some_and(|v| v <= 2);
            !tiny("width") && !tiny("height")
        })
        .filter_map(|img| img.value().attr("src"))
        .find(|s| s.starts_with("http"))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/"><channel>
  <title>Example &amp; Co</title><link>https://example.com/</link><description>Things</description>
  <item>
    <title>Hello &lt;b&gt;world&lt;/b&gt;</title><link>https://example.com/hello</link><guid>h1</guid>
    <pubDate>Mon, 05 Jan 2026 10:00:00 GMT</pubDate>
    <description><![CDATA[<p onclick="x()">Hi <script>alert(1)</script><img src="/pic.jpg"> <a href="/more">more</a></p>]]></description>
  </item>
  <item>
    <title>Podcast</title><guid>p1</guid>
    <enclosure url="https://cdn.example.com/ep.mp3" type="audio/mpeg" length="1"/>
  </item>
</channel></rss>"#;

    #[test]
    fn parses_and_sanitizes_rss() {
        let (meta, items) = parse_feed(RSS.as_bytes(), "https://example.com/feed.xml").unwrap();
        assert_eq!(meta.title, "Example & Co");
        assert_eq!(meta.site_url.as_deref(), Some("https://example.com/"));
        assert_eq!(items.len(), 2);
        let a = &items[0];
        assert_eq!(a.guid, "h1");
        assert_eq!(a.title, "Hello world");
        assert_eq!(a.text, "Hi more");
        assert_eq!(plain_text("<p>One <b>two</b>.</p><p>Three</p>"), "One two. Three");
        assert_eq!(a.published_at.as_deref(), Some("2026-01-05T10:00:00.000Z"));
        assert!(!a.content.contains("script") && !a.content.contains("onclick"));
        assert!(a.content.contains(r#"src="https://example.com/pic.jpg""#), "{}", a.content);
        assert!(a.content.contains(r#"target="_blank""#));
        assert_eq!(a.image_url.as_deref(), Some("https://example.com/pic.jpg"));
        assert!(items[1].content.contains("<audio"), "{}", items[1].content);
    }

    #[test]
    fn parses_atom_and_json_feed() {
        let atom = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Atom</title><link href="https://a.example/"/>
<entry><title>E1</title><id>urn:1</id><updated>2026-02-01T00:00:00Z</updated>
<link href="https://a.example/e1"/><content type="html">&lt;p&gt;Body&lt;/p&gt;</content></entry></feed>"#;
        let (meta, items) = parse_feed(atom.as_bytes(), "https://a.example/atom").unwrap();
        assert_eq!(meta.title, "Atom");
        assert_eq!(items[0].url.as_deref(), Some("https://a.example/e1"));
        assert_eq!(items[0].text, "Body");

        let json = r#"{"version":"https://jsonfeed.org/version/1.1","title":"J","items":[{"id":"1","content_text":"Plain text","url":"https://j.example/1"}]}"#;
        let (meta, items) = parse_feed(json.as_bytes(), "https://j.example/feed.json").unwrap();
        assert_eq!(meta.title, "J");
        assert_eq!(items[0].title, "Plain text", "untitled items fall back to their text");
    }

    #[test]
    fn repairs_unescaped_ampersands() {
        let rss = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Pixel & Ink</title>
<link>https://p.example/?a=1&b=2</link><item><title>Fish &amp; chips &copy; &#169; R&D</title><guid>1</guid></item></channel></rss>"#;
        let (meta, items) = parse_feed(rss.as_bytes(), "https://p.example/feed").unwrap();
        assert_eq!(meta.title, "Pixel & Ink");
        assert_eq!(meta.site_url.as_deref(), Some("https://p.example/?a=1&b=2"));
        assert_eq!(items[0].title, "Fish & chips © © R&D");
        assert!(looks_like_feed_document(rss.as_bytes()));
        assert!(!looks_like_feed_document(b"<!doctype html><html></html>"));
    }

    #[test]
    fn discovers_feeds_in_html() {
        let html = r#"<html><head>
          <link rel="alternate" type="application/rss+xml" title="Comments" href="/comments/feed">
          <link rel="alternate" type="application/atom+xml" title="Main" href="/atom.xml">
          <link rel="stylesheet" href="/s.css"></head></html>"#;
        let found = discover_in_html(html, "https://blog.example/post/1");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].url, "https://blog.example/atom.xml");
        assert_eq!(found[0].kind, "atom");
    }

    #[test]
    fn normalizes_urls() {
        assert_eq!(normalize_url("example.com").unwrap(), "https://example.com/");
        assert_eq!(normalize_url("feed://example.com/rss").unwrap(), "https://example.com/rss");
        assert!(normalize_url("ftp://example.com").is_err());
        assert_eq!(truncate("hello brave new world", 12), "hello brave…");
    }
}
