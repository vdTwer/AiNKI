use std::{path::PathBuf, str::FromStr, sync::Arc};

use anyhow::Context;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use tracing_subscriber::EnvFilter;

mod auth;
mod error;
mod format;
mod import;
mod lists;
mod markdown;
mod mcp;
mod oauth;
mod media;
mod presets;
mod scheduler;
mod security;
mod service;
mod share;
#[cfg(test)]
mod tests;
mod web;

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: String,
    /// Open registration. The very first account can always sign up.
    pub allow_signup: bool,
    pub secure_cookies: bool,
    pub default_timezone: String,
    /// e.g. `https://cards.example.com`: the OAuth issuer and MCP URL.
    /// Derived from the request when unset.
    pub public_url: Option<String>,
}

impl Config {
    /// Served over HTTPS: cookies get the Secure flag and HSTS is sent.
    pub fn secure(&self) -> bool {
        self.secure_cookies || self.public_url.as_deref().is_some_and(|u| u.starts_with("https://"))
    }

    /// `__Host-` cookies are only accepted over HTTPS, for this exact host,
    /// with Path=/ — so a subdomain can't plant or overwrite them.
    pub fn session_cookie_name(&self) -> &'static str {
        if self.secure() { "__Host-ainki_session" } else { "ainki_session" }
    }
}

/// Accept only a bare origin like `https://cards.example.com`.
fn parse_public_url(raw: &str) -> anyhow::Result<String> {
    let url = raw.trim().trim_end_matches('/');
    let (scheme, host) = url
        .split_once("://")
        .ok_or_else(|| anyhow::anyhow!("AINKI_PUBLIC_URL must start with https:// or http://"))?;
    anyhow::ensure!(
        matches!(scheme, "https" | "http"),
        "AINKI_PUBLIC_URL must start with https:// or http://"
    );
    anyhow::ensure!(
        !host.is_empty() && !host.contains(['/', '?', '#', '@', ' ']),
        "AINKI_PUBLIC_URL must be just scheme and host, like https://cards.example.com"
    );
    Ok(format!("{scheme}://{}", host.to_ascii_lowercase()))
}

impl Config {
    fn from_env() -> anyhow::Result<Self> {
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
        Ok(Self {
            data_dir: env("AINKI_DATA_DIR").unwrap_or_else(|| "./data".into()).into(),
            bind: env("AINKI_BIND").unwrap_or_else(|| "0.0.0.0:8080".into()),
            allow_signup: flag("AINKI_ALLOW_SIGNUP", false),
            secure_cookies: flag("AINKI_SECURE_COOKIES", false),
            default_timezone: tz,
            public_url: env("AINKI_PUBLIC_URL").map(|u| parse_public_url(&u)).transpose()?,
        })
    }
}

pub struct App {
    pub db: SqlitePool,
    pub config: Config,
    pub limiter: security::RateLimiter,
}

impl App {
    pub fn new(db: SqlitePool, config: Config) -> Self {
        Self { db, config, limiter: Default::default() }
    }

    /// Whether the signup page is open right now.
    pub async fn signup_open(&self) -> bool {
        if self.config.allow_signup {
            return true;
        }
        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&self.db)
            .await
            .unwrap_or(1);
        users == 0
    }

    /// Delete expired sessions, codes, tokens and leftover import files.
    pub async fn cleanup(&self) -> anyhow::Result<()> {
        let t = now();
        sqlx::query("DELETE FROM sessions WHERE expires_at < ?").bind(t).execute(&self.db).await?;
        sqlx::query("DELETE FROM oauth_codes WHERE expires_at < ?").bind(t).execute(&self.db).await?;
        sqlx::query("DELETE FROM oauth_tokens WHERE expires_at < ?").bind(t).execute(&self.db).await?;
        if let Ok(mut dir) = tokio::fs::read_dir(self.config.data_dir.join("tmp")).await {
            while let Ok(Some(entry)) = dir.next_entry().await {
                let old = entry
                    .metadata()
                    .await
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|m| m.elapsed().ok())
                    .is_some_and(|age| age.as_secs() > 3600);
                if old {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                }
            }
        }
        Ok(())
    }
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
  ainki create-user <name> <password>    create an account
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
    let config = Config::from_env()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("healthcheck") {
        return healthcheck(&config);
    }
    let db = open_db(&config).await?;
    let app = Arc::new(App::new(db, config));

    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] | ["serve"] => {
            if app.config.public_url.is_none() {
                tracing::warn!(
                    "AINKI_PUBLIC_URL is not set; the public address is taken from request headers. \
                     Set it (e.g. https://cards.example.com) before exposing AiNKI or connecting Claude."
                );
            }
            let cleaner = app.clone();
            tokio::spawn(async move {
                loop {
                    if let Err(e) = cleaner.cleanup().await {
                        tracing::warn!("cleanup failed: {e}");
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                }
            });
            web::serve(app).await
        }
        ["create-user", name, password] => {
            let user = app
                .create_user(name, password, &app.config.default_timezone.clone())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("created {}", user.handle());
            Ok(())
        }
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
