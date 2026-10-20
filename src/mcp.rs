//! Model Context Protocol server exposing feed management to AI agents.

use ego_tree::NodeRef;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerConfig},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
};
use scraper::{Html, Node};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    db::{ArticleQuery, CategoryFilter, FeedUpdate, ReadStatus},
    error::AppError,
    feeds::{FeedService, normalize_url},
    opml,
};

const INSTRUCTIONS: &str = "OpenRSS is the user's RSS/Atom feed reader. Use these tools to curate subscriptions \
(add, organize into categories, rename, remove), keep feeds fresh, and read or triage articles \
(search, mark read, star). Start with get_overview to see what exists. add_feed accepts a website URL \
and discovers its feed automatically. Prefer list_articles with status=\"unread\" to find new items, \
and get_article to read one in full (returned as Markdown).";

type ToolResult = Result<String, String>;

fn ok<T: Serialize>(value: T) -> ToolResult {
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}

impl From<AppError> for String {
    fn from(e: AppError) -> Self {
        e.message
    }
}

#[derive(Clone)]
pub struct OpenRssMcp {
    svc: FeedService,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListFeedsArgs {
    /// Only feeds in this category id.
    pub category_id: Option<i64>,
    /// Only feeds without a category.
    #[serde(default)]
    pub uncategorized: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddFeedArgs {
    /// Feed URL or website URL (the feed is auto-discovered). Bare domains are accepted.
    pub url: String,
    /// Category name; created if it does not exist.
    pub category: Option<String>,
    /// Custom display title overriding the feed's own title.
    pub title: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UrlArgs {
    /// Website or feed URL.
    pub url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateFeedArgs {
    pub feed_id: i64,
    /// New display title. Empty string restores the feed's own title.
    pub title: Option<String>,
    /// Category name to move the feed to (created if missing). Empty string removes its category.
    pub category: Option<String>,
    /// New feed URL.
    pub url: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FeedIdArgs {
    pub feed_id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RefreshArgs {
    /// Feeds to refresh. Omit to refresh every feed.
    pub feed_ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NameArgs {
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RenameCategoryArgs {
    pub category_id: i64,
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CategoryIdArgs {
    pub category_id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListArticlesArgs {
    pub feed_id: Option<i64>,
    pub category_id: Option<i64>,
    /// "all" (default), "unread" or "read".
    #[serde(default)]
    pub status: ReadStatus,
    /// Only starred articles.
    #[serde(default)]
    pub starred: bool,
    /// Full-text search over title, author and body.
    pub search: Option<String>,
    /// Page size, 1-100. Default 20.
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// Return oldest articles first.
    #[serde(default)]
    pub oldest_first: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetArticleArgs {
    pub article_id: i64,
    /// "markdown" (default) or "html".
    pub format: Option<String>,
    /// Also mark the article as read.
    #[serde(default)]
    pub mark_read: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MarkArticlesArgs {
    pub article_ids: Vec<i64>,
    /// Set read (true) or unread (false). Omit to leave unchanged.
    pub read: Option<bool>,
    /// Star (true) or unstar (false). Omit to leave unchanged.
    pub starred: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MarkAllReadArgs {
    /// Limit to one feed.
    pub feed_id: Option<i64>,
    /// Limit to one category.
    pub category_id: Option<i64>,
    /// Only articles published at or before this RFC 3339 timestamp.
    pub before: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportOpmlArgs {
    /// The full OPML XML document.
    pub opml: String,
}

#[tool_router]
impl OpenRssMcp {
    pub fn new(svc: FeedService) -> Self {
        Self { svc, tool_router: Self::tool_router() }
    }

    #[tool(
        description = "Summary of the reader: totals, categories and every feed with its unread count and last error.",
        annotations(read_only_hint = true)
    )]
    async fn get_overview(&self) -> ToolResult {
        let store = &self.svc.store;
        let feeds: Vec<_> = store
            .list_feeds(None)?
            .into_iter()
            .map(|f| {
                json!({
                    "id": f.id, "title": f.title, "url": f.url, "category": f.category,
                    "unread": f.unread_count, "last_error": f.last_error,
                })
            })
            .collect();
        ok(json!({ "stats": store.stats()?, "categories": store.list_categories()?, "feeds": feeds }))
    }

    #[tool(description = "List subscribed feeds with full details.", annotations(read_only_hint = true))]
    async fn list_feeds(&self, Parameters(a): Parameters<ListFeedsArgs>) -> ToolResult {
        let filter = if a.uncategorized { Some(CategoryFilter::Uncategorized) } else { a.category_id.map(CategoryFilter::Id) };
        ok(self.svc.store.list_feeds(filter)?)
    }

    #[tool(
        description = "Subscribe to a feed. Accepts a feed URL or a website URL (its feed is discovered). Imports current entries.",
        annotations(read_only_hint = false, destructive_hint = false, open_world_hint = true)
    )]
    async fn add_feed(&self, Parameters(a): Parameters<AddFeedArgs>) -> ToolResult {
        let (feed, counts) = self.svc.add_feed(&a.url, a.category.as_deref(), a.title.as_deref()).await?;
        ok(json!({ "feed": feed, "articles_added": counts.added }))
    }

    #[tool(
        description = "Find the RSS/Atom/JSON feeds offered by a website without subscribing.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn discover_feeds(&self, Parameters(a): Parameters<UrlArgs>) -> ToolResult {
        ok(self.svc.discover(&a.url).await?)
    }

    #[tool(description = "Rename a feed, move it to another category, or change its URL.", annotations(idempotent_hint = true))]
    async fn update_feed(&self, Parameters(a): Parameters<UpdateFeedArgs>) -> ToolResult {
        let store = &self.svc.store;
        let category_id = match a.category {
            Some(name) => Some(store.ensure_category(Some(&name))?),
            None => None,
        };
        let url = a.url.map(|u| normalize_url(&u)).transpose()?;
        ok(store.update_feed(a.feed_id, FeedUpdate { title: a.title, category_id, url })?)
    }

    #[tool(description = "Unsubscribe from a feed and delete its articles (including starred ones).", annotations(destructive_hint = true))]
    async fn remove_feed(&self, Parameters(a): Parameters<FeedIdArgs>) -> ToolResult {
        let feed = self.svc.store.get_feed(a.feed_id)?;
        self.svc.store.delete_feed(a.feed_id)?;
        ok(json!({ "removed": { "id": feed.id, "title": feed.title, "url": feed.url } }))
    }

    #[tool(description = "Fetch new articles now, for some feeds or all of them.", annotations(open_world_hint = true))]
    async fn refresh_feeds(&self, Parameters(a): Parameters<RefreshArgs>) -> ToolResult {
        let mut summary = self.svc.refresh_many(a.feed_ids).await?;
        // Keep the response small: only feeds that changed or failed.
        summary.results.retain(|r| r.added > 0 || r.error.is_some());
        ok(summary)
    }

    #[tool(description = "List categories with feed and unread counts.", annotations(read_only_hint = true))]
    async fn list_categories(&self) -> ToolResult {
        ok(self.svc.store.list_categories()?)
    }

    #[tool(description = "Create a category (returns the existing one if the name is taken).", annotations(idempotent_hint = true))]
    async fn create_category(&self, Parameters(a): Parameters<NameArgs>) -> ToolResult {
        ok(self.svc.store.create_category(&a.name)?)
    }

    #[tool(description = "Rename a category.", annotations(idempotent_hint = true))]
    async fn rename_category(&self, Parameters(a): Parameters<RenameCategoryArgs>) -> ToolResult {
        ok(self.svc.store.update_category(a.category_id, Some(&a.name), None)?)
    }

    #[tool(description = "Delete a category. Its feeds are kept and become uncategorized.", annotations(destructive_hint = true))]
    async fn delete_category(&self, Parameters(a): Parameters<CategoryIdArgs>) -> ToolResult {
        self.svc.store.delete_category(a.category_id)?;
        ok(json!({ "deleted": a.category_id }))
    }

    #[tool(
        description = "List or search articles, newest first. Returns titles, summaries and ids; use get_article for full text.",
        annotations(read_only_hint = true)
    )]
    async fn list_articles(&self, Parameters(a): Parameters<ListArticlesArgs>) -> ToolResult {
        let page = self.svc.store.list_articles(&ArticleQuery {
            feed_id: a.feed_id,
            category: a.category_id.map(CategoryFilter::Id),
            status: a.status,
            starred: a.starred,
            search: a.search,
            limit: a.limit.unwrap_or(20).clamp(1, 100),
            offset: a.offset.unwrap_or(0),
            oldest_first: a.oldest_first,
        })?;
        let articles: Vec<_> = page
            .articles
            .iter()
            .map(|x| {
                json!({
                    "id": x.id, "title": x.title, "feed": x.feed_title, "feed_id": x.feed_id, "url": x.url,
                    "author": x.author, "published_at": x.published_at, "summary": x.summary,
                    "is_read": x.is_read, "is_starred": x.is_starred,
                })
            })
            .collect();
        ok(json!({ "total": page.total, "offset": page.offset, "returned": articles.len(), "articles": articles }))
    }

    #[tool(description = "Read one article in full, as Markdown (default) or sanitized HTML.", annotations(idempotent_hint = true))]
    async fn get_article(&self, Parameters(a): Parameters<GetArticleArgs>) -> ToolResult {
        if a.mark_read {
            self.svc.store.set_article_flags(&[a.article_id], Some(true), None)?;
        }
        let art = self.svc.store.get_article(a.article_id)?;
        let body = if a.format.as_deref() == Some("html") { art.content.clone() } else { html_to_markdown(&art.content) };
        let m = art.meta;
        ok(json!({
            "id": m.id, "title": m.title, "feed": m.feed_title, "feed_id": m.feed_id, "url": m.url,
            "author": m.author, "published_at": m.published_at, "is_read": m.is_read, "is_starred": m.is_starred,
            "content": body,
        }))
    }

    #[tool(description = "Mark articles read/unread and/or starred/unstarred.", annotations(idempotent_hint = true))]
    async fn mark_articles(&self, Parameters(a): Parameters<MarkArticlesArgs>) -> ToolResult {
        if a.read.is_none() && a.starred.is_none() {
            return Err("Provide read and/or starred".into());
        }
        let n = self.svc.store.set_article_flags(&a.article_ids, a.read, a.starred)?;
        ok(json!({ "updated": n }))
    }

    #[tool(description = "Mark all unread articles as read, optionally limited to a feed, category or date.", annotations(idempotent_hint = true))]
    async fn mark_all_read(&self, Parameters(a): Parameters<MarkAllReadArgs>) -> ToolResult {
        let n = self.svc.store.mark_all_read(a.feed_id, a.category_id.map(CategoryFilter::Id), a.before.as_deref())?;
        ok(json!({ "updated": n }))
    }

    #[tool(description = "Subscribe to every feed in an OPML document, keeping its folders as categories.", annotations(open_world_hint = true))]
    async fn import_opml(&self, Parameters(a): Parameters<ImportOpmlArgs>) -> ToolResult {
        ok(self.svc.import_opml(&a.opml).await?)
    }

    #[tool(description = "Export all subscriptions as an OPML document.", annotations(read_only_hint = true))]
    async fn export_opml(&self) -> ToolResult {
        Ok(opml::export(&self.svc.store.list_feeds(None)?, &self.svc.store.list_categories()?))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for OpenRssMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("openrss", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

/// Converts sanitized article HTML to compact Markdown for language models.
pub fn html_to_markdown(html: &str) -> String {
    let frag = Html::parse_fragment(html);
    let mut out = String::new();
    walk(frag.tree.root(), &mut out, 0);
    // Collapse runs of blank lines.
    let mut result = String::with_capacity(out.len());
    let mut blank = 0;
    for line in out.lines().map(str::trim_end) {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        result.push_str(line);
        result.push('\n');
    }
    result.trim().to_string()
}

fn inline_text(node: NodeRef<Node>) -> String {
    let mut s = String::new();
    walk(node, &mut s, 0);
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn walk(node: NodeRef<Node>, out: &mut String, depth: usize) {
    for child in node.children() {
        match child.value() {
            Node::Text(t) => {
                let text = t.text.replace(['\n', '\t'], " ");
                if !(text.trim().is_empty() && (out.is_empty() || out.ends_with('\n'))) {
                    out.push_str(&text);
                }
            }
            Node::Element(el) => {
                let name = el.name();
                match name {
                    "script" | "style" | "audio" | "video" => {
                        if let Some(src) = el.attr("src") {
                            out.push_str(&format!("\n\n[{name}]({src})\n\n"));
                        }
                    }
                    "br" => out.push('\n'),
                    "hr" => out.push_str("\n\n---\n\n"),
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        let level = name[1..].parse::<usize>().unwrap_or(2);
                        out.push_str(&format!("\n\n{} {}\n\n", "#".repeat(level), inline_text(child)));
                    }
                    "a" => {
                        let text = inline_text(child);
                        match el.attr("href") {
                            Some(href) if !text.is_empty() && !href.starts_with('#') => out.push_str(&format!("[{text}]({href})")),
                            _ => out.push_str(&text),
                        }
                    }
                    "img" => {
                        if let Some(src) = el.attr("src") {
                            out.push_str(&format!("![{}]({src})", el.attr("alt").unwrap_or("").trim()));
                        }
                    }
                    "strong" | "b" => out.push_str(&format!("**{}**", inline_text(child))),
                    "em" | "i" => out.push_str(&format!("*{}*", inline_text(child))),
                    "code" if !matches!(child.parent().and_then(|p| p.value().as_element().map(|e| e.name() == "pre")), Some(true)) => {
                        out.push_str(&format!("`{}`", inline_text(child)));
                    }
                    "pre" => {
                        let code: String = scraper::ElementRef::wrap(child).map(|e| e.text().collect()).unwrap_or_default();
                        out.push_str(&format!("\n\n```\n{}\n```\n\n", code.trim_end()));
                    }
                    "li" => {
                        out.push_str(&format!("\n{}- ", "  ".repeat(depth.saturating_sub(1))));
                        walk(child, out, depth);
                    }
                    "ul" | "ol" => {
                        out.push('\n');
                        walk(child, out, depth + 1);
                        out.push('\n');
                    }
                    "blockquote" => {
                        let mut inner = String::new();
                        walk(child, &mut inner, depth);
                        let quoted: Vec<String> = inner.trim().lines().map(|l| format!("> {}", l.trim())).collect();
                        out.push_str(&format!("\n\n{}\n\n", quoted.join("\n")));
                    }
                    "p" | "div" | "section" | "article" | "figure" | "figcaption" | "table" | "tr" | "header" | "footer"
                    | "aside" | "details" | "summary" | "main" | "nav" => {
                        out.push_str("\n\n");
                        walk(child, out, depth);
                        out.push_str("\n\n");
                    }
                    "td" | "th" => {
                        walk(child, out, depth);
                        out.push_str(" | ");
                    }
                    _ => walk(child, out, depth),
                }
            }
            _ => walk(child, out, depth),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_html_to_markdown() {
        let html = r#"<h2>Title</h2><p>Some <strong>bold</strong> and a <a href="https://x.y/">link</a>.</p>
            <ul><li>one</li><li>two</li></ul><pre><code>let x = 1;</code></pre><blockquote><p>quote</p></blockquote>"#;
        let md = html_to_markdown(html);
        assert_eq!(md, "## Title\n\nSome **bold** and a [link](https://x.y/).\n\n- one\n- two\n\n```\nlet x = 1;\n```\n\n> quote");
    }
}
