use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter, types::Value};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::error::{AppError, AppResult};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS categories (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE COLLATE NOCASE,
  position INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS feeds (
  id INTEGER PRIMARY KEY,
  url TEXT NOT NULL UNIQUE,
  title TEXT NOT NULL DEFAULT '',
  custom_title TEXT,
  site_url TEXT,
  description TEXT,
  category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
  etag TEXT,
  last_modified TEXT,
  last_fetched_at TEXT,
  last_error TEXT,
  error_count INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS articles (
  id INTEGER PRIMARY KEY,
  feed_id INTEGER NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
  guid TEXT NOT NULL,
  url TEXT,
  title TEXT NOT NULL DEFAULT '',
  author TEXT,
  summary TEXT NOT NULL DEFAULT '',
  content TEXT NOT NULL DEFAULT '',
  image_url TEXT,
  published_at TEXT NOT NULL,
  fetched_at TEXT NOT NULL,
  is_read INTEGER NOT NULL DEFAULT 0,
  is_starred INTEGER NOT NULL DEFAULT 0,
  UNIQUE (feed_id, guid)
);

CREATE INDEX IF NOT EXISTS idx_articles_published ON articles (published_at DESC);
CREATE INDEX IF NOT EXISTS idx_articles_feed ON articles (feed_id, published_at DESC);
CREATE INDEX IF NOT EXISTS idx_articles_unread ON articles (is_read, published_at DESC);
CREATE INDEX IF NOT EXISTS idx_articles_starred ON articles (is_starred, published_at DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS articles_fts USING fts5 (
  title, author, body, tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS articles_fts_delete AFTER DELETE ON articles BEGIN
  DELETE FROM articles_fts WHERE rowid = old.id;
END;

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS themes (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE COLLATE NOCASE,
  base TEXT NOT NULL DEFAULT 'light',
  vars TEXT NOT NULL DEFAULT '{}',
  custom_css TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
"#;

pub fn now() -> String {
    format_time(chrono::Utc::now())
}

/// All timestamps are stored as UTC RFC 3339 with millisecond precision so
/// that lexical order equals chronological order.
pub fn format_time(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[derive(Debug, Clone, Serialize)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub position: i64,
    pub feed_count: i64,
    pub unread_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Feed {
    pub id: i64,
    pub url: String,
    /// Display title: the custom title if set, otherwise the feed's own title.
    pub title: String,
    pub custom_title: Option<String>,
    pub site_url: Option<String>,
    pub description: Option<String>,
    pub category_id: Option<i64>,
    pub category: Option<String>,
    pub last_fetched_at: Option<String>,
    pub last_error: Option<String>,
    pub error_count: i64,
    pub created_at: String,
    pub unread_count: i64,
    pub article_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleSummary {
    pub id: i64,
    pub feed_id: i64,
    pub feed_title: String,
    pub feed_site_url: Option<String>,
    pub url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub summary: String,
    pub image_url: Option<String>,
    pub published_at: String,
    pub is_read: bool,
    pub is_starred: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Article {
    #[serde(flatten)]
    pub meta: ArticleSummary,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticlePage {
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub articles: Vec<ArticleSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub feeds: i64,
    pub categories: i64,
    pub articles: i64,
    pub unread: i64,
    pub starred: i64,
    pub feeds_with_errors: i64,
    pub last_refresh: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    #[serde(default = "default_base")]
    pub base: String,
    #[serde(default)]
    pub vars: HashMap<String, String>,
    #[serde(default)]
    pub custom_css: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

fn default_base() -> String {
    "light".into()
}

/// Feed metadata learned from a successful fetch.
#[derive(Debug, Default, Clone)]
pub struct FeedMeta {
    pub title: String,
    pub site_url: Option<String>,
    pub description: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// A normalized feed entry, ready to be stored.
#[derive(Debug, Default, Clone)]
pub struct NewArticle {
    pub guid: String,
    pub url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub summary: String,
    pub content: String,
    /// Plain text used for full-text search.
    pub text: String,
    pub image_url: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct UpsertCounts {
    pub added: usize,
    pub updated: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReadStatus {
    #[default]
    All,
    Unread,
    Read,
}

/// Filter for the "no category" pseudo-category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryFilter {
    Id(i64),
    Uncategorized,
}

#[derive(Debug, Clone, Default)]
pub struct ArticleQuery {
    pub feed_id: Option<i64>,
    pub category: Option<CategoryFilter>,
    pub status: ReadStatus,
    pub starred: bool,
    pub search: Option<String>,
    pub limit: i64,
    pub offset: i64,
    pub oldest_first: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FeedUpdate {
    pub title: Option<String>,
    /// `Some(None)` removes the feed from its category.
    pub category_id: Option<Option<i64>>,
    pub url: Option<String>,
}

const FEED_SELECT: &str = r#"
  SELECT f.id, f.url, COALESCE(NULLIF(f.custom_title, ''), NULLIF(f.title, ''), f.url) AS title,
         f.custom_title, f.site_url, f.description, f.category_id, c.name AS category,
         f.last_fetched_at, f.last_error, f.error_count, f.created_at,
         (SELECT COUNT(*) FROM articles a WHERE a.feed_id = f.id AND a.is_read = 0) AS unread_count,
         (SELECT COUNT(*) FROM articles a WHERE a.feed_id = f.id) AS article_count
  FROM feeds f LEFT JOIN categories c ON c.id = f.category_id
"#;

const ARTICLE_COLUMNS: &str = r#"
  a.id, a.feed_id, COALESCE(NULLIF(f.custom_title, ''), NULLIF(f.title, ''), f.url) AS feed_title,
  f.site_url, a.url, a.title, a.author, a.summary, a.image_url, a.published_at, a.is_read, a.is_starred
"#;

fn feed_from_row(r: &Row) -> rusqlite::Result<Feed> {
    Ok(Feed {
        id: r.get(0)?,
        url: r.get(1)?,
        title: r.get(2)?,
        custom_title: r.get(3)?,
        site_url: r.get(4)?,
        description: r.get(5)?,
        category_id: r.get(6)?,
        category: r.get(7)?,
        last_fetched_at: r.get(8)?,
        last_error: r.get(9)?,
        error_count: r.get(10)?,
        created_at: r.get(11)?,
        unread_count: r.get(12)?,
        article_count: r.get(13)?,
    })
}

fn article_from_row(r: &Row) -> rusqlite::Result<ArticleSummary> {
    Ok(ArticleSummary {
        id: r.get(0)?,
        feed_id: r.get(1)?,
        feed_title: r.get(2)?,
        feed_site_url: r.get(3)?,
        url: r.get(4)?,
        title: r.get(5)?,
        author: r.get(6)?,
        summary: r.get(7)?,
        image_url: r.get(8)?,
        published_at: r.get(9)?,
        is_read: r.get(10)?,
        is_starred: r.get(11)?,
    })
}

fn theme_from_row(r: &Row) -> rusqlite::Result<Theme> {
    let vars: String = r.get(3)?;
    Ok(Theme {
        id: r.get(0)?,
        name: r.get(1)?,
        base: r.get(2)?,
        vars: serde_json::from_str(&vars).unwrap_or_default(),
        custom_css: r.get(4)?,
        created_at: r.get(5)?,
    })
}

/// Turn free text into a safe FTS5 query: each term quoted and prefix-matched.
fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

/// Thread-safe handle to the SQLite database.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> anyhow::Result<Self> {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---------- categories ----------

    pub fn list_categories(&self) -> AppResult<Vec<Category>> {
        let c = self.conn();
        let mut stmt = c.prepare_cached(
            r#"SELECT c.id, c.name, c.position,
                 (SELECT COUNT(*) FROM feeds f WHERE f.category_id = c.id),
                 (SELECT COUNT(*) FROM articles a JOIN feeds f ON f.id = a.feed_id
                    WHERE f.category_id = c.id AND a.is_read = 0)
               FROM categories c ORDER BY c.position, c.name COLLATE NOCASE"#,
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Category {
                id: r.get(0)?,
                name: r.get(1)?,
                position: r.get(2)?,
                feed_count: r.get(3)?,
                unread_count: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get_category(&self, id: i64) -> AppResult<Category> {
        self.list_categories()?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| AppError::not_found(format!("Category {id} not found")))
    }

    pub fn find_category_by_name(&self, name: &str) -> AppResult<Option<i64>> {
        Ok(self.conn().query_row("SELECT id FROM categories WHERE name = ?", [name.trim()], |r| r.get(0)).optional()?)
    }

    /// Creates the category, or returns the existing one with the same name.
    pub fn create_category(&self, name: &str) -> AppResult<Category> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::bad_request("Category name is required"));
        }
        if let Some(id) = self.find_category_by_name(name)? {
            return self.get_category(id);
        }
        let id = {
            let c = self.conn();
            c.execute(
                "INSERT INTO categories (name, position) VALUES (?, (SELECT COALESCE(MAX(position), 0) + 1 FROM categories))",
                [name],
            )?;
            c.last_insert_rowid()
        };
        self.get_category(id)
    }

    /// Resolves a category name to an id, creating it when needed. Blank means none.
    pub fn ensure_category(&self, name: Option<&str>) -> AppResult<Option<i64>> {
        match name.map(str::trim) {
            None | Some("") => Ok(None),
            Some(n) => Ok(Some(self.create_category(n)?.id)),
        }
    }

    pub fn update_category(&self, id: i64, name: Option<&str>, position: Option<i64>) -> AppResult<Category> {
        self.get_category(id)?;
        if let Some(name) = name {
            let name = name.trim();
            if name.is_empty() {
                return Err(AppError::bad_request("Category name is required"));
            }
            if matches!(self.find_category_by_name(name)?, Some(other) if other != id) {
                return Err(AppError::conflict("A category with that name already exists"));
            }
            self.conn().execute("UPDATE categories SET name = ? WHERE id = ?", params![name, id])?;
        }
        if let Some(p) = position {
            self.conn().execute("UPDATE categories SET position = ? WHERE id = ?", params![p, id])?;
        }
        self.get_category(id)
    }

    /// Deletes a category; its feeds become uncategorized.
    pub fn delete_category(&self, id: i64) -> AppResult<()> {
        let n = self.conn().execute("DELETE FROM categories WHERE id = ?", [id])?;
        if n == 0 {
            return Err(AppError::not_found(format!("Category {id} not found")));
        }
        Ok(())
    }

    // ---------- feeds ----------

    pub fn list_feeds(&self, category: Option<CategoryFilter>) -> AppResult<Vec<Feed>> {
        let c = self.conn();
        let (sql, param): (String, Option<i64>) = match category {
            None => (format!("{FEED_SELECT} ORDER BY title COLLATE NOCASE"), None),
            Some(CategoryFilter::Uncategorized) => {
                (format!("{FEED_SELECT} WHERE f.category_id IS NULL ORDER BY title COLLATE NOCASE"), None)
            }
            Some(CategoryFilter::Id(id)) => {
                (format!("{FEED_SELECT} WHERE f.category_id = ? ORDER BY title COLLATE NOCASE"), Some(id))
            }
        };
        let mut stmt = c.prepare_cached(&sql)?;
        let rows = stmt.query_map(params_from_iter(param), feed_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get_feed(&self, id: i64) -> AppResult<Feed> {
        self.conn()
            .query_row(&format!("{FEED_SELECT} WHERE f.id = ?"), [id], feed_from_row)
            .optional()?
            .ok_or_else(|| AppError::not_found(format!("Feed {id} not found")))
    }

    pub fn find_feed_by_url(&self, url: &str) -> AppResult<Option<Feed>> {
        Ok(self.conn().query_row(&format!("{FEED_SELECT} WHERE f.url = ?"), [url], feed_from_row).optional()?)
    }

    /// Returns (url, etag, last_modified) for conditional fetching.
    pub fn feed_fetch_state(&self, id: i64) -> AppResult<(String, Option<String>, Option<String>)> {
        self.conn()
            .query_row("SELECT url, etag, last_modified FROM feeds WHERE id = ?", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?
            .ok_or_else(|| AppError::not_found(format!("Feed {id} not found")))
    }

    pub fn feed_ids(&self) -> AppResult<Vec<i64>> {
        let c = self.conn();
        let mut stmt = c.prepare_cached("SELECT id FROM feeds ORDER BY id")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn insert_feed(
        &self,
        url: &str,
        meta: &FeedMeta,
        category_id: Option<i64>,
        custom_title: Option<&str>,
    ) -> AppResult<i64> {
        let c = self.conn();
        c.execute(
            r#"INSERT INTO feeds (url, title, site_url, description, category_id, custom_title, etag, last_modified, last_fetched_at, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)"#,
            params![
                url,
                meta.title,
                meta.site_url,
                meta.description,
                category_id,
                custom_title.map(str::trim).filter(|s| !s.is_empty()),
                meta.etag,
                meta.last_modified,
                now()
            ],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn update_feed(&self, id: i64, update: FeedUpdate) -> AppResult<Feed> {
        let feed = self.get_feed(id)?;
        if let Some(title) = update.title {
            let t = title.trim();
            self.conn()
                .execute("UPDATE feeds SET custom_title = ? WHERE id = ?", params![(!t.is_empty()).then_some(t), id])?;
        }
        if let Some(cat) = update.category_id {
            if let Some(cid) = cat {
                self.get_category(cid)?;
            }
            self.conn().execute("UPDATE feeds SET category_id = ? WHERE id = ?", params![cat, id])?;
        }
        if let Some(url) = update.url.filter(|u| *u != feed.url) {
            if self.find_feed_by_url(&url)?.is_some() {
                return Err(AppError::conflict("Another feed already uses that URL"));
            }
            self.conn().execute(
                "UPDATE feeds SET url = ?, etag = NULL, last_modified = NULL WHERE id = ?",
                params![url, id],
            )?;
        }
        self.get_feed(id)
    }

    pub fn delete_feed(&self, id: i64) -> AppResult<()> {
        let n = self.conn().execute("DELETE FROM feeds WHERE id = ?", [id])?;
        if n == 0 {
            return Err(AppError::not_found(format!("Feed {id} not found")));
        }
        Ok(())
    }

    pub fn record_fetch_success(&self, id: i64, meta: &FeedMeta) -> AppResult<()> {
        self.conn().execute(
            r#"UPDATE feeds SET
                 title = COALESCE(NULLIF(?1, ''), title),
                 site_url = COALESCE(NULLIF(?2, ''), site_url),
                 description = COALESCE(NULLIF(?3, ''), description),
                 etag = ?4, last_modified = ?5, last_fetched_at = ?6, last_error = NULL, error_count = 0
               WHERE id = ?7"#,
            params![
                meta.title,
                meta.site_url.as_deref().unwrap_or(""),
                meta.description.as_deref().unwrap_or(""),
                meta.etag,
                meta.last_modified,
                now(),
                id
            ],
        )?;
        Ok(())
    }

    pub fn record_fetch_not_modified(&self, id: i64) -> AppResult<()> {
        self.conn().execute(
            "UPDATE feeds SET last_fetched_at = ?, last_error = NULL, error_count = 0 WHERE id = ?",
            params![now(), id],
        )?;
        Ok(())
    }

    pub fn record_fetch_error(&self, id: i64, message: &str) -> AppResult<()> {
        let msg: String = message.chars().take(500).collect();
        self.conn().execute(
            "UPDATE feeds SET last_fetched_at = ?, last_error = ?, error_count = error_count + 1 WHERE id = ?",
            params![now(), msg, id],
        )?;
        Ok(())
    }

    // ---------- articles ----------

    /// Inserts new entries and updates changed ones. Read/star state is preserved.
    pub fn upsert_articles(&self, feed_id: i64, items: &[NewArticle]) -> AppResult<UpsertCounts> {
        let fetched_at = now();
        let mut counts = UpsertCounts::default();
        let mut c = self.conn();
        let tx = c.transaction()?;
        {
            let mut upsert = tx.prepare_cached(
                r#"INSERT INTO articles (feed_id, guid, url, title, author, summary, content, image_url, published_at, fetched_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                   ON CONFLICT (feed_id, guid) DO UPDATE SET
                     url = excluded.url, title = excluded.title, author = excluded.author,
                     summary = excluded.summary, content = excluded.content, image_url = excluded.image_url
                   WHERE articles.title IS NOT excluded.title OR articles.content IS NOT excluded.content
                   RETURNING id"#,
            )?;
            let mut exists = tx.prepare_cached("SELECT 1 FROM articles WHERE feed_id = ? AND guid = ?")?;
            let mut fts_del = tx.prepare_cached("DELETE FROM articles_fts WHERE rowid = ?")?;
            let mut fts_ins =
                tx.prepare_cached("INSERT INTO articles_fts (rowid, title, author, body) VALUES (?, ?, ?, ?)")?;
            for it in items {
                let published = it.published_at.clone().unwrap_or_else(|| fetched_at.clone());
                let is_new = !exists.exists(params![feed_id, it.guid])?;
                let id: Option<i64> = upsert
                    .query_row(
                        params![
                            feed_id,
                            it.guid,
                            it.url,
                            it.title,
                            it.author,
                            it.summary,
                            it.content,
                            it.image_url,
                            published,
                            fetched_at
                        ],
                        |r| r.get(0),
                    )
                    .optional()?;
                // No row: the entry exists and is unchanged.
                let Some(id) = id else { continue };
                if is_new {
                    counts.added += 1;
                } else {
                    counts.updated += 1;
                }
                fts_del.execute([id])?;
                fts_ins.execute(params![id, it.title, it.author.as_deref().unwrap_or(""), it.text])?;
            }
        }
        tx.commit()?;
        Ok(counts)
    }

    pub fn list_articles(&self, q: &ArticleQuery) -> AppResult<ArticlePage> {
        let mut wheres: Vec<&str> = Vec::new();
        let mut args: Vec<Value> = Vec::new();
        let mut from = String::from("articles a JOIN feeds f ON f.id = a.feed_id");
        if let Some(id) = q.feed_id {
            wheres.push("a.feed_id = ?");
            args.push(id.into());
        }
        match q.category {
            Some(CategoryFilter::Id(id)) => {
                wheres.push("f.category_id = ?");
                args.push(id.into());
            }
            Some(CategoryFilter::Uncategorized) => wheres.push("f.category_id IS NULL"),
            None => {}
        }
        match q.status {
            ReadStatus::Unread => wheres.push("a.is_read = 0"),
            ReadStatus::Read => wheres.push("a.is_read = 1"),
            ReadStatus::All => {}
        }
        if q.starred {
            wheres.push("a.is_starred = 1");
        }
        if let Some(fts) = q.search.as_deref().and_then(fts_query) {
            from.push_str(" JOIN articles_fts ON articles_fts.rowid = a.id");
            wheres.push("articles_fts MATCH ?");
            args.push(fts.into());
        }
        let where_sql = if wheres.is_empty() { String::new() } else { format!("WHERE {}", wheres.join(" AND ")) };
        let dir = if q.oldest_first { "ASC" } else { "DESC" };
        let limit = q.limit.clamp(1, 500);
        let offset = q.offset.max(0);

        let c = self.conn();
        let total: i64 =
            c.query_row(&format!("SELECT COUNT(*) FROM {from} {where_sql}"), params_from_iter(args.iter()), |r| {
                r.get(0)
            })?;
        let sql = format!(
            "SELECT {ARTICLE_COLUMNS} FROM {from} {where_sql} ORDER BY a.published_at {dir}, a.id {dir} LIMIT {limit} OFFSET {offset}"
        );
        let mut stmt = c.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), article_from_row)?;
        Ok(ArticlePage { total, limit, offset, articles: rows.collect::<Result<_, _>>()? })
    }

    pub fn get_article(&self, id: i64) -> AppResult<Article> {
        self.conn()
            .query_row(
                &format!("SELECT {ARTICLE_COLUMNS}, a.content FROM articles a JOIN feeds f ON f.id = a.feed_id WHERE a.id = ?"),
                [id],
                |r| Ok(Article { meta: article_from_row(r)?, content: r.get(12)? }),
            )
            .optional()?
            .ok_or_else(|| AppError::not_found(format!("Article {id} not found")))
    }

    /// Sets read and/or starred flags. Returns the number of matched articles.
    pub fn set_article_flags(&self, ids: &[i64], read: Option<bool>, starred: Option<bool>) -> AppResult<usize> {
        let ids: Vec<i64> = ids.iter().copied().collect::<HashSet<_>>().into_iter().collect();
        let mut c = self.conn();
        let tx = c.transaction()?;
        let mut matched = 0;
        {
            let mut set_read = tx.prepare_cached("UPDATE articles SET is_read = ? WHERE id = ?")?;
            let mut set_star = tx.prepare_cached("UPDATE articles SET is_starred = ? WHERE id = ?")?;
            let mut exists = tx.prepare_cached("SELECT 1 FROM articles WHERE id = ?")?;
            for id in ids {
                if !exists.exists([id])? {
                    continue;
                }
                matched += 1;
                if let Some(v) = read {
                    set_read.execute(params![v, id])?;
                }
                if let Some(v) = starred {
                    set_star.execute(params![v, id])?;
                }
            }
        }
        tx.commit()?;
        Ok(matched)
    }

    /// Marks every unread article in scope as read. Returns how many changed.
    pub fn mark_all_read(
        &self,
        feed_id: Option<i64>,
        category: Option<CategoryFilter>,
        before: Option<&str>,
    ) -> AppResult<usize> {
        let mut wheres = vec!["is_read = 0"];
        let mut args: Vec<Value> = Vec::new();
        if let Some(id) = feed_id {
            wheres.push("feed_id = ?");
            args.push(id.into());
        }
        match category {
            Some(CategoryFilter::Id(id)) => {
                wheres.push("feed_id IN (SELECT id FROM feeds WHERE category_id = ?)");
                args.push(id.into());
            }
            Some(CategoryFilter::Uncategorized) => {
                wheres.push("feed_id IN (SELECT id FROM feeds WHERE category_id IS NULL)")
            }
            None => {}
        }
        if let Some(b) = before {
            wheres.push("published_at <= ?");
            args.push(b.to_string().into());
        }
        let sql = format!("UPDATE articles SET is_read = 1 WHERE {}", wheres.join(" AND "));
        Ok(self.conn().execute(&sql, params_from_iter(args.iter()))?)
    }

    /// Deletes old read, unstarred articles, always keeping each feed's newest 50.
    pub fn prune_articles(&self, days: u32) -> AppResult<usize> {
        if days == 0 {
            return Ok(0);
        }
        let cutoff = format_time(chrono::Utc::now() - chrono::Duration::days(days as i64));
        Ok(self.conn().execute(
            r#"DELETE FROM articles WHERE is_read = 1 AND is_starred = 0 AND published_at < ?
                 AND id NOT IN (
                   SELECT id FROM (
                     SELECT id, ROW_NUMBER() OVER (PARTITION BY feed_id ORDER BY published_at DESC) AS rn FROM articles
                   ) WHERE rn <= 50
                 )"#,
            [cutoff],
        )?)
    }

    pub fn stats(&self) -> AppResult<Stats> {
        Ok(self.conn().query_row(
            r#"SELECT
                 (SELECT COUNT(*) FROM feeds),
                 (SELECT COUNT(*) FROM categories),
                 (SELECT COUNT(*) FROM articles),
                 (SELECT COUNT(*) FROM articles WHERE is_read = 0),
                 (SELECT COUNT(*) FROM articles WHERE is_starred = 1),
                 (SELECT COUNT(*) FROM feeds WHERE last_error IS NOT NULL),
                 (SELECT MAX(last_fetched_at) FROM feeds)"#,
            [],
            |r| {
                Ok(Stats {
                    feeds: r.get(0)?,
                    categories: r.get(1)?,
                    articles: r.get(2)?,
                    unread: r.get(3)?,
                    starred: r.get(4)?,
                    feeds_with_errors: r.get(5)?,
                    last_refresh: r.get(6)?,
                })
            },
        )?)
    }

    // ---------- settings ----------

    pub fn get_settings(&self) -> AppResult<serde_json::Map<String, Json>> {
        let c = self.conn();
        let mut stmt = c.prepare_cached("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = serde_json::Map::new();
        for row in rows {
            let (k, v) = row?;
            if let Ok(v) = serde_json::from_str(&v) {
                out.insert(k, v);
            }
        }
        Ok(out)
    }

    /// Merges a patch into the settings; `null` values delete keys.
    pub fn set_settings(&self, patch: &serde_json::Map<String, Json>) -> AppResult<serde_json::Map<String, Json>> {
        {
            let mut c = self.conn();
            let tx = c.transaction()?;
            for (k, v) in patch {
                if k.len() > 64 {
                    return Err(AppError::bad_request("Setting key too long"));
                }
                if v.is_null() {
                    tx.execute("DELETE FROM settings WHERE key = ?", [k])?;
                } else {
                    tx.execute(
                        "INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                        params![k, serde_json::to_string(v)?],
                    )?;
                }
            }
            tx.commit()?;
        }
        self.get_settings()
    }

    // ---------- themes ----------

    pub fn list_themes(&self) -> AppResult<Vec<Theme>> {
        let c = self.conn();
        let mut stmt = c.prepare_cached(
            "SELECT id, name, base, vars, custom_css, created_at FROM themes ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], theme_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get_theme(&self, id: i64) -> AppResult<Theme> {
        self.conn()
            .query_row(
                "SELECT id, name, base, vars, custom_css, created_at FROM themes WHERE id = ?",
                [id],
                theme_from_row,
            )
            .optional()?
            .ok_or_else(|| AppError::not_found(format!("Theme {id} not found")))
    }

    /// Creates (no id) or updates a custom theme after validating its contents.
    pub fn save_theme(&self, theme: Theme) -> AppResult<Theme> {
        let name = theme.name.trim().to_string();
        if name.is_empty() || name.chars().count() > 60 {
            return Err(AppError::bad_request("Theme name must be 1-60 characters"));
        }
        let base = if theme.base == "dark" { "dark" } else { "light" };
        let vars: HashMap<String, String> = theme
            .vars
            .into_iter()
            .filter(|(k, v)| {
                k.len() <= 64
                    && k.starts_with("--")
                    && k[2..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                    && v.len() <= 200
                    && !v.contains(['{', '}', ';', '<', '>'])
            })
            .collect();
        if theme.custom_css.len() > 50_000 {
            return Err(AppError::bad_request("Custom CSS is too long (max 50 KB)"));
        }
        // The CSS is injected into a <style> element; never let it close that element.
        let css = theme.custom_css.replace("</", "<\\/");
        let vars_json = serde_json::to_string(&vars)?;

        let existing: Option<i64> =
            self.conn().query_row("SELECT id FROM themes WHERE name = ?", [&name], |r| r.get(0)).optional()?;
        if existing.is_some() && existing != theme.id {
            return Err(AppError::conflict("A theme with that name already exists"));
        }
        let id = match theme.id {
            Some(id) => {
                let n = self.conn().execute(
                    "UPDATE themes SET name = ?, base = ?, vars = ?, custom_css = ? WHERE id = ?",
                    params![name, base, vars_json, css, id],
                )?;
                if n == 0 {
                    return Err(AppError::not_found(format!("Theme {id} not found")));
                }
                id
            }
            None => {
                let c = self.conn();
                c.execute(
                    "INSERT INTO themes (name, base, vars, custom_css, created_at) VALUES (?, ?, ?, ?, ?)",
                    params![name, base, vars_json, css, now()],
                )?;
                c.last_insert_rowid()
            }
        };
        self.get_theme(id)
    }

    pub fn delete_theme(&self, id: i64) -> AppResult<()> {
        let n = self.conn().execute("DELETE FROM themes WHERE id = ?", [id])?;
        if n == 0 {
            return Err(AppError::not_found(format!("Theme {id} not found")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn article(guid: &str, title: &str, published: &str) -> NewArticle {
        NewArticle {
            guid: guid.into(),
            title: title.into(),
            content: format!("<p>{title}</p>"),
            text: format!("{title} body text"),
            published_at: Some(published.into()),
            ..Default::default()
        }
    }

    #[test]
    fn feeds_articles_and_search() {
        let s = Store::open_in_memory().unwrap();
        let cat = s.create_category("Tech").unwrap();
        assert_eq!(s.create_category(" tech ").unwrap().id, cat.id, "names are case-insensitive");
        let fid = s
            .insert_feed(
                "https://example.com/feed",
                &FeedMeta { title: "Example".into(), ..Default::default() },
                Some(cat.id),
                None,
            )
            .unwrap();
        let counts = s
            .upsert_articles(
                fid,
                &[
                    article("1", "Rust is fast", "2026-01-01T00:00:00.000Z"),
                    article("2", "Café culture", "2026-01-02T00:00:00.000Z"),
                ],
            )
            .unwrap();
        assert_eq!(counts.added, 2);

        // Re-upserting unchanged entries is a no-op; changed ones count as updates.
        let counts = s.upsert_articles(fid, &[article("1", "Rust is fast", "2026-01-01T00:00:00.000Z")]).unwrap();
        assert_eq!((counts.added, counts.updated), (0, 0));
        let counts = s.upsert_articles(fid, &[article("1", "Rust is very fast", "2026-01-01T00:00:00.000Z")]).unwrap();
        assert_eq!((counts.added, counts.updated), (0, 1));

        let page = s.list_articles(&ArticleQuery { limit: 10, ..Default::default() }).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.articles[0].title, "Café culture", "newest first");

        let found =
            s.list_articles(&ArticleQuery { search: Some("cafe".into()), limit: 10, ..Default::default() }).unwrap();
        assert_eq!(found.total, 1, "diacritics are ignored by search");
        let found =
            s.list_articles(&ArticleQuery { search: Some("ver".into()), limit: 10, ..Default::default() }).unwrap();
        assert_eq!(found.total, 1, "prefix search");

        let id = page.articles[0].id;
        assert_eq!(s.set_article_flags(&[id, id, 9999], Some(true), Some(true)).unwrap(), 1);
        let a = s.get_article(id).unwrap();
        assert!(a.meta.is_read && a.meta.is_starred);
        let unread =
            s.list_articles(&ArticleQuery { status: ReadStatus::Unread, limit: 10, ..Default::default() }).unwrap();
        assert_eq!(unread.total, 1);
        assert_eq!(s.mark_all_read(None, Some(CategoryFilter::Id(cat.id)), None).unwrap(), 1);
        assert_eq!(s.stats().unwrap().unread, 0);

        s.delete_category(cat.id).unwrap();
        assert_eq!(s.get_feed(fid).unwrap().category_id, None, "feeds survive category deletion");
        s.delete_feed(fid).unwrap();
        assert_eq!(s.stats().unwrap().articles, 0, "articles cascade with their feed");
    }

    #[test]
    fn themes_are_sanitized() {
        let s = Store::open_in_memory().unwrap();
        let mut vars = HashMap::new();
        vars.insert("--bg".to_string(), "#fff".to_string());
        vars.insert("--evil".to_string(), "red} body{display:none".to_string());
        vars.insert("color".to_string(), "red".to_string());
        let t = s
            .save_theme(Theme {
                id: None,
                name: "Mine".into(),
                base: "weird".into(),
                vars,
                custom_css: "</style><script>alert(1)</script>".into(),
                created_at: None,
            })
            .unwrap();
        assert_eq!(t.base, "light");
        assert_eq!(t.vars.len(), 1);
        assert!(!t.custom_css.contains("</"));
        assert!(s.save_theme(Theme { id: None, ..t.clone() }).is_err(), "duplicate names rejected");
    }
}
