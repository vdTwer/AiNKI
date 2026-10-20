use std::{path::PathBuf, str::FromStr, sync::Arc};

use anyhow::Context;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use tracing_subscriber::EnvFilter;

mod auth;
mod error;
mod import;
mod markdown;
mod mcp;
mod oauth;
mod offline;
mod media;
mod presets;
mod scheduler;
mod service;
mod share;
#[cfg(test)]
mod tests;
mod web;

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: String,
    pub allow_signup: bool,
    pub secure_cookies: bool,
    pub default_timezone: String,
    /// Used to show the MCP URL on the tokens page. Derived from the request
    /// when unset.
    pub public_url: Option<String>,
}

impl Config {
    fn from_env() -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let flag = |k: &str, default: bool| {
            env(k)
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(default)
        };
        let tz = env("AINKI_TIMEZONE")
            .or_else(|| env("TZ"))
            .filter(|t| t.parse::<chrono_tz::Tz>().is_ok())
            .unwrap_or_else(|| "UTC".into());
        Self {
            data_dir: env("AINKI_DATA_DIR").unwrap_or_else(|| "./data".into()).into(),
            bind: env("AINKI_BIND").unwrap_or_else(|| "0.0.0.0:8080".into()),
            allow_signup: flag("AINKI_ALLOW_SIGNUP", true),
            secure_cookies: flag("AINKI_SECURE_COOKIES", false),
            default_timezone: tz,
            public_url: env("AINKI_PUBLIC_URL").map(|u| u.trim_end_matches('/').to_string()),
        }
    }
}

pub struct App {
    pub db: SqlitePool,
    pub config: Config,
}

pub type AppState = Arc<App>;

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

async fn open_db(config: &Config) -> anyhow::Result<SqlitePool> {
    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("creating data dir {}", config.data_dir.display()))?;
    let path = config.data_dir.join("ainki.db");
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(10));
    let pool = SqlitePoolOptions::new().max_connections(8).connect_with(opts).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// Used by the Docker HEALTHCHECK; the slim image has no curl.
fn healthcheck(config: &Config) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    let port = config.bind.rsplit(':').next().unwrap_or("8080");
    let mut s = std::net::TcpStream::connect(format!("127.0.0.1:{port}"))?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
    s.write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut buf = String::new();
    s.read_to_string(&mut buf)?;
    anyhow::ensure!(buf.starts_with("HTTP/1.0 200") || buf.starts_with("HTTP/1.1 200"), "unhealthy");
    Ok(())
}

const USAGE: &str = "usage:
  ainki                                  run the server
  ainki reset-password <name#tag> <new>  set a user's password
  ainki list-users                       list all accounts
  ainki healthcheck                      exit 0 if the server answers";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
        )
        .init();
    let config = Config::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("healthcheck") {
        return healthcheck(&config);
    }
    let db = open_db(&config).await?;
    let app = Arc::new(App { db, config });

    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] | ["serve"] => web::serve(app).await,
        ["reset-password", handle, password] => {
            let user = app.find_user(handle).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            app.set_password(user.id, password).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            sqlx::query("DELETE FROM sessions WHERE user_id = ?")
                .bind(user.id)
                .execute(&app.db)
                .await?;
            println!("password updated for {}", user.handle());
            Ok(())
        }
        ["list-users"] => {
            let users: Vec<auth::User> = sqlx::query_as("SELECT * FROM users ORDER BY id")
                .fetch_all(&app.db)
                .await?;
            for u in users {
                println!("{}", u.handle());
            }
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}
