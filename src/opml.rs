//! OPML import and export.

use quick_xml::{Reader, XmlVersion, events::Event};
use serde::Serialize;

use crate::{
    db::{Category, Feed},
    error::{AppError, AppResult},
    feeds::html_escape,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpmlFeed {
    pub url: String,
    pub title: Option<String>,
    pub category: Option<String>,
}

/// Extracts feed subscriptions from an OPML document. Nested folders are
/// flattened to their outermost name, since categories are one level deep.
pub fn parse(xml: &str) -> AppResult<Vec<OpmlFeed>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut feeds = Vec::new();
    // One entry per open <outline>: the folder name it introduces, if any.
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut saw_opml = false;
    loop {
        let event = reader.read_event().map_err(|e| AppError::bad_request(format!("Invalid OPML: {e}")))?;
        let (e, is_empty) = match &event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(e) => {
                if e.local_name().as_ref().eq_ignore_ascii_case("outline") {
                    stack.pop();
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let name = e.local_name();
        if name.as_ref().eq_ignore_ascii_case("opml") {
            saw_opml = true;
        }
        if !name.as_ref().eq_ignore_ascii_case("outline") {
            continue;
        }
        let mut url = None;
        let mut text = None;
        let mut title = None;
        for attr in e.attributes().flatten() {
            let value = attr
                .normalized_value(XmlVersion::Implicit1_0)
                .map(|v| v.trim().to_string())
                .unwrap_or_default();
            if value.is_empty() {
                continue;
            }
            match attr.key.local_name().as_ref().to_ascii_lowercase().as_str() {
                "xmlurl" => url = Some(value),
                "text" => text = Some(value),
                "title" => title = Some(value),
                _ => {}
            }
        }
        let label = title.or(text);
        let category = stack.iter().flatten().next().cloned();
        match url {
            Some(url) => {
                feeds.push(OpmlFeed { url, title: label, category });
                if !is_empty {
                    stack.push(None);
                }
            }
            None if !is_empty => stack.push(label),
            None => {}
        }
    }
    if !saw_opml {
        return Err(AppError::bad_request("Invalid OPML: missing <opml> root element"));
    }
    Ok(feeds)
}

pub fn export(feeds: &[Feed], categories: &[Category]) -> String {
    let outline = |f: &Feed, indent: &str| {
        let title = html_escape(&f.title);
        let mut line = format!(r#"{indent}<outline type="rss" text="{title}" title="{title}" xmlUrl="{}""#, html_escape(&f.url));
        if let Some(site) = &f.site_url {
            line.push_str(&format!(r#" htmlUrl="{}""#, html_escape(site)));
        }
        line.push_str("/>\n");
        line
    };
    let mut body = String::new();
    for c in categories {
        let name = html_escape(&c.name);
        body.push_str(&format!("    <outline text=\"{name}\" title=\"{name}\">\n"));
        for f in feeds.iter().filter(|f| f.category_id == Some(c.id)) {
            body.push_str(&outline(f, "      "));
        }
        body.push_str("    </outline>\n");
    }
    for f in feeds.iter().filter(|f| f.category_id.is_none()) {
        body.push_str(&outline(f, "    "));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">\n  <head>\n    <title>OpenRSS subscriptions</title>\n    <dateCreated>{}</dateCreated>\n  </head>\n  <body>\n{body}  </body>\n</opml>\n",
        chrono::Utc::now().to_rfc2822()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_opml() {
        let xml = r#"<?xml version="1.0"?>
<opml version="1.0"><head><title>x</title></head><body>
  <outline text="Tech">
    <outline text="Rust" type="rss" xmlUrl="https://blog.rust-lang.org/feed.xml"/>
    <outline text="Deep"><outline title="HN" xmlUrl="https://news.ycombinator.com/rss"/></outline>
  </outline>
  <outline text="Loose &amp; free" xmlUrl="https://example.com/feed"/>
</body></opml>"#;
        let feeds = parse(xml).unwrap();
        assert_eq!(
            feeds,
            vec![
                OpmlFeed { url: "https://blog.rust-lang.org/feed.xml".into(), title: Some("Rust".into()), category: Some("Tech".into()) },
                OpmlFeed { url: "https://news.ycombinator.com/rss".into(), title: Some("HN".into()), category: Some("Tech".into()) },
                OpmlFeed { url: "https://example.com/feed".into(), title: Some("Loose & free".into()), category: None },
            ]
        );
        assert!(parse("<html></html>").is_err());
    }
}
