//! End-to-end test: a local site serves a feed, and OpenRSS is driven over
//! its REST API and MCP endpoint.

use std::sync::Arc;

use axum::{Router, http::header, routing::get};
use openrss::{AppState, config::Config, db::Store, feeds::FeedService};
use serde_json::{Value, json};

const TOKEN: &str = "test-token";

const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Test Site</title><link>https://site.example/</link>
<item><title>Hello OpenRSS</title><link>https://site.example/hello</link><guid>a1</guid>
<pubDate>Mon, 05 Oct 2026 10:00:00 GMT</pubDate><description>&lt;p&gt;Hello &lt;b&gt;reader&lt;/b&gt;.&lt;/p&gt;</description></item>
<item><title>Second item</title><link>https://site.example/second</link><guid>a2</guid>
<pubDate>Tue, 06 Oct 2026 10:00:00 GMT</pubDate><description>About gardening.</description></item>
</channel></rss>"#;

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

async fn mcp_call(client: &reqwest::Client, base: &str, tool: &str, args: Value) -> Value {
    let res: Value = client
        .post(format!("{base}/mcp"))
        .bearer_auth(TOKEN)
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .json(&json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": tool, "arguments": args } }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let result = &res["result"];
    assert_ne!(result["isError"], json!(true), "{tool} failed: {result}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn rest_and_mcp_end_to_end() {
    // Requests to the local test servers must not go through a proxy.
    // SAFETY: set before any other thread reads the environment.
    unsafe { std::env::set_var("NO_PROXY", "127.0.0.1,localhost") };

    let site = serve(
        Router::new()
            .route("/feed.xml", get(|| async { ([(header::CONTENT_TYPE, "application/rss+xml")], RSS) }))
            .route(
                "/",
                get(|| async {
                    axum::response::Html(r#"<html><head><link rel="alternate" type="application/rss+xml" href="/feed.xml"></head></html>"#)
                }),
            ),
    )
    .await;

    let mut config = Config::from_env();
    config.api_token = Some(TOKEN.into());
    config.auth_password = None;
    let store = Store::open_in_memory().unwrap();
    let svc = FeedService::new(store, &config).unwrap();
    let app = openrss::app(AppState { svc, config: Arc::new(config) });
    let base = serve(app).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    // Subscribe through the REST API using the site's home page (feed discovery).
    let res = client
        .post(format!("{base}/api/feeds"))
        .json(&json!({ "url": format!("{site}/"), "category": "News" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["feed"]["url"], format!("{site}/feed.xml"));
    assert_eq!(body["feed"]["category"], "News");
    assert_eq!(body["articles_added"], 2);

    // Subscribing twice is a conflict.
    let res = client
        .post(format!("{base}/api/feeds"))
        .json(&json!({ "url": format!("{site}/feed.xml") }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);

    // The web UI is served, unknown API routes are JSON 404s.
    let res = client.get(format!("{base}/")).send().await.unwrap();
    assert!(res.text().await.unwrap().contains("OpenRSS"));
    assert_eq!(client.get(format!("{base}/api/nope")).send().await.unwrap().status(), 404);

    // OPML export contains the subscription inside its category.
    let opml = client.get(format!("{base}/api/opml")).send().await.unwrap().text().await.unwrap();
    assert!(opml.contains(r#"<outline text="News""#) && opml.contains("/feed.xml"));

    // The MCP endpoint requires the token.
    let res = client.post(format!("{base}/mcp")).json(&json!({})).send().await.unwrap();
    assert_eq!(res.status(), 401);

    let overview = mcp_call(&client, &base, "get_overview", json!({})).await;
    assert_eq!(overview["stats"]["unread"], 2);

    let found = mcp_call(&client, &base, "list_articles", json!({ "search": "gardening" })).await;
    assert_eq!(found["total"], 1);

    let unread = mcp_call(&client, &base, "list_articles", json!({ "status": "unread", "oldest_first": true })).await;
    let first_id = unread["articles"][0]["id"].as_i64().unwrap();
    assert_eq!(unread["articles"][0]["title"], "Hello OpenRSS");

    let article = mcp_call(&client, &base, "get_article", json!({ "article_id": first_id, "mark_read": true })).await;
    assert_eq!(article["content"], "Hello **reader**.");
    assert_eq!(article["is_read"], true);

    mcp_call(&client, &base, "mark_articles", json!({ "article_ids": [first_id], "starred": true })).await;
    let starred = mcp_call(&client, &base, "list_articles", json!({ "starred": true })).await;
    assert_eq!(starred["total"], 1);

    // Agents can reorganize subscriptions.
    let feed_id = body["feed"]["id"].as_i64().unwrap();
    let updated = mcp_call(
        &client,
        &base,
        "update_feed",
        json!({ "feed_id": feed_id, "category": "Hobbies", "title": "My site" }),
    )
    .await;
    assert_eq!(updated["category"], "Hobbies");
    assert_eq!(updated["title"], "My site");

    let refreshed = mcp_call(&client, &base, "refresh_feeds", json!({})).await;
    assert_eq!(refreshed["errors"], 0);

    mcp_call(&client, &base, "remove_feed", json!({ "feed_id": feed_id })).await;
    let stats: Value = client.get(format!("{base}/api/stats")).send().await.unwrap().json().await.unwrap();
    assert_eq!(stats["feeds"], 0);
    assert_eq!(stats["articles"], 0);
}
