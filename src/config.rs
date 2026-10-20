use std::{env, path::PathBuf, time::Duration};

/// Runtime configuration, read from environment variables.
#[derive(Clone, Debug)]
pub struct Config {
    pub bind: String,
    pub data_dir: PathBuf,
    /// Interval between automatic refreshes of every feed.
    pub refresh_interval: Duration,
    /// Days to keep read, unstarred articles. 0 disables cleanup.
    pub retention_days: u32,
    pub fetch_timeout: Duration,
    pub fetch_concurrency: usize,
    pub user_agent: String,
    /// Optional HTTP basic auth for the web UI and REST API.
    pub auth_user: String,
    pub auth_password: Option<String>,
    /// Optional bearer token, required by /mcp and accepted by /api.
    pub api_token: Option<String>,
    pub mcp_enabled: bool,
    /// Hostnames accepted by the MCP endpoint (DNS-rebinding protection). Empty = any.
    pub mcp_allowed_hosts: Vec<String>,
}

fn var(name: &str) -> Option<String> {
    env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn num<T: std::str::FromStr>(name: &str, default: T) -> T {
    var(name).and_then(|v| v.parse().ok()).unwrap_or(default)
}

impl Config {
    pub fn from_env() -> Self {
        let port: u16 = num("PORT", 8080);
        let host = var("HOST").unwrap_or_else(|| "0.0.0.0".into());
        Self {
            bind: var("BIND").unwrap_or_else(|| format!("{host}:{port}")),
            data_dir: PathBuf::from(var("DATA_DIR").unwrap_or_else(|| "./data".into())),
            refresh_interval: Duration::from_secs(60 * num::<u64>("REFRESH_INTERVAL", 30).max(1)),
            retention_days: num("RETENTION_DAYS", 60),
            fetch_timeout: Duration::from_secs(num::<u64>("FETCH_TIMEOUT", 20).max(1)),
            fetch_concurrency: num::<usize>("FETCH_CONCURRENCY", 6).max(1),
            user_agent: var("USER_AGENT").unwrap_or_else(|| {
                format!("OpenRSS/{} (+https://github.com/pmorcrette/openrss)", env!("CARGO_PKG_VERSION"))
            }),
            auth_user: var("AUTH_USER").unwrap_or_else(|| "admin".into()),
            auth_password: var("AUTH_PASSWORD"),
            api_token: var("API_TOKEN"),
            mcp_enabled: var("MCP_ENABLED").map(|v| v.to_lowercase() != "false").unwrap_or(true),
            mcp_allowed_hosts: var("MCP_ALLOWED_HOSTS")
                .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                .unwrap_or_default(),
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("openrss.db")
    }
}
