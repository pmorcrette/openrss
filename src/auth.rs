//! Optional authentication: HTTP basic auth for people, bearer tokens for agents.

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::Engine;

use crate::AppState;

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn bearer_ok(headers: &HeaderMap, token: Option<&str>) -> bool {
    let Some(token) = token else { return false };
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|given| constant_time_eq(given.trim().as_bytes(), token.as_bytes()))
}

fn basic_ok(headers: &HeaderMap, user: &str, password: Option<&str>) -> bool {
    let Some(password) = password else { return false };
    let Some(encoded) =
        headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Basic "))
    else {
        return false;
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else { return false };
    let expected = format!("{user}:{password}");
    constant_time_eq(&decoded, expected.as_bytes())
}

fn unauthorized(basic: bool) -> Response {
    let mut res = (StatusCode::UNAUTHORIZED, "Authentication required").into_response();
    if basic {
        res.headers_mut()
            .insert(header::WWW_AUTHENTICATE, "Basic realm=\"OpenRSS\", charset=\"UTF-8\"".parse().unwrap());
    }
    res
}

/// Guards the web UI and REST API when `AUTH_PASSWORD` is set. The API token
/// is accepted too, so scripts can use the REST API.
pub async fn require_user(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let c = &s.config;
    if c.auth_password.is_none() || req.uri().path() == "/api/health" {
        return next.run(req).await;
    }
    let h = req.headers();
    if basic_ok(h, &c.auth_user, c.auth_password.as_deref()) || bearer_ok(h, c.api_token.as_deref()) {
        return next.run(req).await;
    }
    unauthorized(true)
}

/// Guards the MCP endpoint: requires `API_TOKEN` when set, otherwise falls back
/// to the web credentials, otherwise is open.
pub async fn require_agent(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let c = &s.config;
    let h = req.headers();
    let ok = match (&c.api_token, &c.auth_password) {
        (None, None) => true,
        (token, password) => bearer_ok(h, token.as_deref()) || basic_ok(h, &c.auth_user, password.as_deref()),
    };
    if ok { next.run(req).await } else { unauthorized(false) }
}
