//! OAuth 2.1 authorization server for MCP clients (the MCP authorization spec).
//!
//! Connecting Claude: add a custom connector with `https://<host>/mcp`, log
//! in, click Allow. Behind that:
//! - `/.well-known/oauth-protected-resource` (RFC 9728) points clients here,
//! - `/.well-known/oauth-authorization-server` (RFC 8414) lists the endpoints,
//! - clients register dynamically (RFC 7591) or identify themselves with a
//!   client metadata document URL,
//! - authorization code flow with mandatory PKCE (S256),
//! - 1-hour access tokens and rotating refresh tokens.
//!
//! An approved connection is stored like a manual API token, so it appears
//! (and can be revoked) on the Agent access page.

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::Form;
use base64::Engine;
use maud::html;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    App, AppState,
    auth::{random_hex, sha256_hex},
    error::{AppError, Result},
    now,
    web::{WebUser, layout, urlenc},
};

pub const ACCESS_TOKEN_SECS: i64 = 3600;
pub const REFRESH_TOKEN_SECS: i64 = 90 * 86_400;
const CODE_SECS: i64 = 600;
const REFRESH_REUSE_GRACE_SECS: i64 = 30;

/// The server's public base URL, e.g. `https://cards.example.com`.
pub fn base_url(app: &App, headers: &HeaderMap) -> String {
    if let Some(u) = &app.config.public_url {
        return u.clone();
    }
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost:8080");
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("http");
    format!("{proto}://{host}")
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Client {
    pub id: String,
    pub secret_hash: Option<String>,
    pub name: String,
    pub redirect_uris: String,
    pub metadata_url: Option<String>,
    pub created_at: i64,
}

impl Client {
    fn redirects(&self) -> Vec<String> {
        serde_json::from_str(&self.redirect_uris).unwrap_or_default()
    }

    /// Exact match, except that loopback redirects may use any port
    /// (RFC 8252: native apps pick a free port at runtime).
    fn allows_redirect(&self, uri: &str) -> bool {
        self.redirects().iter().any(|r| r == uri || same_loopback(r, uri))
    }
}

fn same_loopback(registered: &str, requested: &str) -> bool {
    let strip = |u: &str| -> Option<(String, String)> {
        let rest = u.strip_prefix("http://")?;
        let (hostport, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
        let host = hostport.rsplit_once(':').map(|(h, _)| h).unwrap_or(hostport);
        matches!(host, "localhost" | "127.0.0.1" | "[::1]").then(|| (host.to_string(), path))
    };
    matches!((strip(registered), strip(requested)), (Some(a), Some(b)) if a == b)
}

fn valid_redirect(uri: &str) -> bool {
    let lower = uri.to_ascii_lowercase();
    let Some((scheme, rest)) = lower.split_once(':') else { return false };
    if rest.is_empty() || uri.contains('#') {
        return false;
    }
    match scheme {
        "https" => true,
        // Plain http only for apps on the user's own machine.
        "http" => same_loopback(uri, uri),
        "javascript" | "data" | "file" | "vbscript" | "about" | "blob" | "filesystem" | "view-source"
        | "intent" | "ws" | "wss" | "ftp" => false,
        // Custom schemes for native apps (e.g. com.example.app:/callback).
        s => s.contains('.') || s.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-'),
    }
}

fn pkce_ok(verifier: &str, challenge: &str) -> bool {
    if !(43..=128).contains(&verifier.len()) {
        return false;
    }
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest) == challenge
}

#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
    pub refresh_token: String,
    pub scope: String,
}

/// Errors in the shape OAuth clients expect.
#[derive(Debug)]
pub struct OAuthError {
    status: StatusCode,
    error: &'static str,
    description: String,
}

impl OAuthError {
    fn new(error: &'static str, description: impl Into<String>) -> Self {
        let status = if error == "invalid_client" { StatusCode::UNAUTHORIZED } else { StatusCode::BAD_REQUEST };
        Self { status, error, description: description.into() }
    }
}

impl From<AppError> for OAuthError {
    fn from(e: AppError) -> Self {
        tracing::error!("oauth: {e}");
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, error: "server_error", description: "internal error".into() }
    }
}

impl From<sqlx::Error> for OAuthError {
    fn from(e: sqlx::Error) -> Self {
        AppError::from(e).into()
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        (
            self.status,
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({ "error": self.error, "error_description": self.description })),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct RegistrationRequest {
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    pub client_name: Option<String>,
    pub token_endpoint_auth_method: Option<String>,
}

/// Client metadata document fetched from a URL client_id.
#[derive(Debug, Deserialize)]
struct MetadataDocument {
    client_id: String,
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
}

/// Only addresses on the public internet: never loopback, private,
/// link-local, carrier-grade NAT, multicast, documentation or reserved ranges.
fn is_public_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
                || (o[0] == 198 && (o[1] & 0xfe) == 18)
                || o[0] >= 240
                || (o[0] == 192 && o[1] == 0 && o[2] == 0))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00
                || (seg[0] & 0xffc0) == 0xfe80
                || (seg[0] == 0x2001 && seg[1] == 0x0db8)
                || (seg[0] == 0x64 && seg[1] == 0xff9b)
                || seg[0] == 0x2002)
        }
    }
}

const MAX_METADATA_BYTES: usize = 64 * 1024;

/// Fetch a client metadata document. The host must resolve only to public
/// addresses, and the connection is pinned to the checked address so DNS
/// can't be switched in between (rebinding).
async fn fetch_metadata_document(url: &str) -> Option<MetadataDocument> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some_and(|p| p != 443)
        || parsed.path() == "/"
        || parsed.fragment().is_some()
    {
        return None;
    }
    let host = parsed.host_str()?.to_string();
    let addrs: Vec<std::net::SocketAddr> = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::lookup_host((host.trim_matches(['[', ']']), 443)),
    )
    .await
    .ok()?
    .ok()?
    .collect();
    if addrs.is_empty() || !addrs.iter().all(|a| is_public_ip(a.ip())) {
        tracing::warn!("refusing to fetch client metadata from non-public host {host}");
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, addrs[0])
        .no_proxy()
        .build()
        .ok()?;
    let mut resp = client.get(parsed).header("Accept", "application/json").send().await.ok()?;
    if !resp.status().is_success() || resp.content_length().is_some_and(|l| l as usize > MAX_METADATA_BYTES) {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_METADATA_BYTES {
            return None;
        }
    }
    serde_json::from_slice(&body).ok()
}

/// Compare secrets without leaking how many leading bytes matched.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Strip control characters and cap the length of client-supplied names.
fn clean_name(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(100).collect::<String>().trim().to_string()
}

impl App {
    pub async fn register_client(&self, req: RegistrationRequest) -> Result<Value, OAuthError> {
        if req.redirect_uris.is_empty() || req.redirect_uris.len() > 20 {
            return Err(OAuthError::new("invalid_redirect_uri", "give 1 to 20 redirect_uris"));
        }
        if let Some(bad) = req.redirect_uris.iter().find(|u| u.len() > 2000 || !valid_redirect(u)) {
            return Err(OAuthError::new("invalid_redirect_uri", format!("redirect URI not allowed: {bad}")));
        }
        let method = req.token_endpoint_auth_method.as_deref().unwrap_or("none");
        if !matches!(method, "none" | "client_secret_post" | "client_secret_basic") {
            return Err(OAuthError::new("invalid_client_metadata", "unsupported token_endpoint_auth_method"));
        }
        let mut name = clean_name(req.client_name.as_deref().unwrap_or(""));
        if name.is_empty() {
            name = "MCP client".into();
        }
        let id = format!("ainki-client-{}", random_hex(16));
        let secret = (method != "none").then(|| random_hex(32));
        sqlx::query(
            "INSERT INTO oauth_clients (id, secret_hash, name, redirect_uris, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(secret.as_deref().map(sha256_hex))
        .bind(&name)
        .bind(serde_json::to_string(&req.redirect_uris).unwrap_or_default())
        .bind(now())
        .execute(&self.db)
        .await?;
        let mut out = json!({
            "client_id": id,
            "client_id_issued_at": now(),
            "client_name": name,
            "redirect_uris": req.redirect_uris,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": method,
        });
        if let Some(s) = secret {
            out["client_secret"] = json!(s);
            out["client_secret_expires_at"] = json!(0);
        }
        Ok(out)
    }

    /// Look up a registered client, or fetch the metadata document for a
    /// URL-shaped client_id.
    pub async fn client(&self, client_id: &str) -> Result<Option<Client>> {
        let stored: Option<Client> = sqlx::query_as("SELECT * FROM oauth_clients WHERE id = ?")
            .bind(client_id)
            .fetch_optional(&self.db)
            .await?;
        let is_url = client_id.starts_with("https://");
        match stored {
            Some(c) if !is_url || c.created_at > now() - 86_400 => Ok(Some(c)),
            _ if is_url => self.fetch_client_metadata(client_id).await,
            _ => Ok(None),
        }
    }

    async fn fetch_client_metadata(&self, url: &str) -> Result<Option<Client>> {
        // Anyone can make us fetch a URL here, so bound how often.
        if self.limiter.check_and_hit("cimd-fetch", 300, std::time::Duration::from_secs(3600)) {
            tracing::warn!("client metadata fetches throttled");
            return Ok(None);
        }
        let fetched = fetch_metadata_document(url).await;
        let Some(doc) = fetched else {
            tracing::warn!("could not fetch OAuth client metadata from {url}");
            return Ok(None);
        };
        if doc.client_id != url
            || doc.redirect_uris.is_empty()
            || doc.redirect_uris.len() > 20
            || !doc.redirect_uris.iter().all(|u| u.len() <= 2000 && valid_redirect(u))
        {
            tracing::warn!("invalid OAuth client metadata document at {url}");
            return Ok(None);
        }
        let host = url.trim_start_matches("https://").split('/').next().unwrap_or(url).to_string();
        let name = doc.client_name.map(|n| clean_name(&n)).filter(|n| !n.is_empty()).unwrap_or(host);
        sqlx::query(
            "INSERT INTO oauth_clients (id, name, redirect_uris, metadata_url, created_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name, redirect_uris = excluded.redirect_uris,
                created_at = excluded.created_at",
        )
        .bind(url)
        .bind(name.chars().take(100).collect::<String>())
        .bind(serde_json::to_string(&doc.redirect_uris).unwrap_or_default())
        .bind(url)
        .bind(now())
        .execute(&self.db)
        .await?;
        Ok(sqlx::query_as("SELECT * FROM oauth_clients WHERE id = ?")
            .bind(url)
            .fetch_optional(&self.db)
            .await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_auth_code(
        &self,
        user_id: i64,
        client: &Client,
        redirect_uri: &str,
        code_challenge: &str,
        can_write: bool,
        can_review: bool,
        decks: Option<Vec<i64>>,
    ) -> Result<String> {
        let code = random_hex(32);
        sqlx::query(
            "INSERT INTO oauth_codes (code_hash, client_id, user_id, redirect_uri, code_challenge,
                can_write, can_review, decks, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(sha256_hex(&code))
        .bind(&client.id)
        .bind(user_id)
        .bind(redirect_uri)
        .bind(code_challenge)
        .bind(can_write)
        .bind(can_review)
        .bind(decks.map(|d| serde_json::to_string(&d).unwrap_or_default()))
        .bind(now() + CODE_SECS)
        .execute(&self.db)
        .await?;
        Ok(code)
    }

    /// Check client credentials. Public clients (no secret) are fine; PKCE
    /// protects their codes.
    async fn authenticate_client(&self, client_id: &str, secret: Option<&str>) -> Result<Client, OAuthError> {
        let client = self
            .client(client_id)
            .await?
            .ok_or_else(|| OAuthError::new("invalid_client", "unknown client"))?;
        if let Some(hash) = &client.secret_hash {
            let given = secret.map(sha256_hex).unwrap_or_default();
            if !ct_eq(given.as_bytes(), hash.as_bytes()) {
                return Err(OAuthError::new("invalid_client", "wrong client secret"));
            }
        }
        Ok(client)
    }

    async fn issue_tokens(&self, grant_id: i64, scope: String) -> Result<TokenResponse> {
        let access = format!("ainki_at_{}", random_hex(24));
        let refresh = format!("ainki_rt_{}", random_hex(32));
        let mut tx = self.db.begin().await?;
        // Expired tokens are garbage.
        sqlx::query("DELETE FROM oauth_tokens WHERE expires_at < ?")
            .bind(now())
            .execute(&mut *tx)
            .await?;
        for (token, kind, ttl) in [(&access, "access", ACCESS_TOKEN_SECS), (&refresh, "refresh", REFRESH_TOKEN_SECS)] {
            sqlx::query("INSERT INTO oauth_tokens (token_hash, kind, api_token_id, expires_at) VALUES (?, ?, ?, ?)")
                .bind(sha256_hex(token))
                .bind(kind)
                .bind(grant_id)
                .bind(now() + ttl)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(TokenResponse {
            access_token: access,
            token_type: "Bearer",
            expires_in: ACCESS_TOKEN_SECS,
            refresh_token: refresh,
            scope,
        })
    }

    async fn grant_scope(&self, grant_id: i64) -> Result<String> {
        let (w, r): (bool, bool) = sqlx::query_as("SELECT can_write, can_review FROM api_tokens WHERE id = ?")
            .bind(grant_id)
            .fetch_one(&self.db)
            .await?;
        Ok(scope_string(w, r))
    }

    pub async fn exchange_code(
        &self,
        client_id: &str,
        client_secret: Option<&str>,
        code: &str,
        redirect_uri: Option<&str>,
        verifier: &str,
    ) -> Result<TokenResponse, OAuthError> {
        let client = self.authenticate_client(client_id, client_secret).await?;
        #[derive(sqlx::FromRow)]
        struct Code {
            client_id: String,
            user_id: i64,
            redirect_uri: String,
            code_challenge: String,
            can_write: bool,
            can_review: bool,
            decks: Option<String>,
            expires_at: i64,
        }
        // Codes are single use: delete on read.
        let row: Option<Code> = sqlx::query_as("DELETE FROM oauth_codes WHERE code_hash = ? RETURNING *")
            .bind(sha256_hex(code))
            .fetch_optional(&self.db)
            .await?;
        let invalid = || OAuthError::new("invalid_grant", "invalid or expired authorization code");
        let row = row.ok_or_else(invalid)?;
        if row.client_id != client.id || row.expires_at < now() {
            return Err(invalid());
        }
        if redirect_uri.is_some_and(|r| r != row.redirect_uri) {
            return Err(OAuthError::new("invalid_grant", "redirect_uri does not match"));
        }
        if !pkce_ok(verifier, &row.code_challenge) {
            return Err(OAuthError::new("invalid_grant", "PKCE verification failed"));
        }
        let decks: Option<Vec<i64>> = row.decks.as_deref().and_then(|d| serde_json::from_str(d).ok());
        let grant = self
            .create_grant(
                row.user_id,
                &client.name,
                row.can_write,
                row.can_review,
                decks,
                &random_hex(32),
                Some(&client.id),
            )
            .await
            .map_err(|e| match e {
                AppError::NotFound | AppError::BadRequest(_) => OAuthError::new("invalid_grant", "decks no longer exist"),
                e => e.into(),
            })?;
        Ok(self.issue_tokens(grant.id, scope_string(row.can_write, row.can_review)).await?)
    }

    pub async fn refresh_tokens(
        &self,
        client_id: &str,
        client_secret: Option<&str>,
        refresh_token: &str,
    ) -> Result<TokenResponse, OAuthError> {
        let client = self.authenticate_client(client_id, client_secret).await?;
        let hash = sha256_hex(refresh_token);
        let invalid = || OAuthError::new("invalid_grant", "invalid or expired refresh token");
        // Rotation: the old refresh token is marked used (kept to spot reuse).
        let row: Option<(i64, i64)> = sqlx::query_as(
            "UPDATE oauth_tokens SET kind = 'refresh_used', used_at = ?
             WHERE token_hash = ? AND kind = 'refresh' RETURNING api_token_id, expires_at",
        )
        .bind(now())
        .bind(&hash)
        .fetch_optional(&self.db)
        .await?;
        let Some((grant_id, expires_at)) = row else {
            // A used refresh token coming back means two parties hold it:
            // the token was probably stolen. End the connection. A quick
            // retry (e.g. the response got lost) is tolerated.
            let used: Option<(i64, i64)> = sqlx::query_as(
                "SELECT api_token_id, used_at FROM oauth_tokens WHERE token_hash = ? AND kind = 'refresh_used'",
            )
            .bind(&hash)
            .fetch_optional(&self.db)
            .await?;
            if let Some((grant, used_at)) = used {
                if now() - used_at > REFRESH_REUSE_GRACE_SECS {
                    tracing::warn!("refresh token reuse detected; revoking connection {grant}");
                    sqlx::query("UPDATE api_tokens SET revoked_at = ? WHERE id = ?")
                        .bind(now())
                        .bind(grant)
                        .execute(&self.db)
                        .await?;
                    sqlx::query("DELETE FROM oauth_tokens WHERE api_token_id = ?")
                        .bind(grant)
                        .execute(&self.db)
                        .await?;
                }
            }
            return Err(invalid());
        };
        let owner: Option<Option<String>> = sqlx::query_scalar(
            "SELECT oauth_client_id FROM api_tokens WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(grant_id)
        .fetch_optional(&self.db)
        .await?;
        if expires_at < now() || owner.flatten().as_deref() != Some(client.id.as_str()) {
            return Err(invalid());
        }
        let scope = self.grant_scope(grant_id).await?;
        Ok(self.issue_tokens(grant_id, scope).await?)
    }

    /// RFC 7009: revoking either token ends the whole connection.
    pub async fn revoke_oauth_token(&self, token: &str) -> Result<()> {
        let grant: Option<i64> = sqlx::query_scalar("SELECT api_token_id FROM oauth_tokens WHERE token_hash = ?")
            .bind(sha256_hex(token))
            .fetch_optional(&self.db)
            .await?;
        if let Some(id) = grant {
            sqlx::query("UPDATE api_tokens SET revoked_at = ? WHERE id = ?")
                .bind(now())
                .bind(id)
                .execute(&self.db)
                .await?;
            sqlx::query("DELETE FROM oauth_tokens WHERE api_token_id = ?")
                .bind(id)
                .execute(&self.db)
                .await?;
        }
        Ok(())
    }
}

fn scope_string(can_write: bool, can_review: bool) -> String {
    let mut s = vec!["read"];
    if can_write {
        s.push("write");
    }
    if can_review {
        s.push("review");
    }
    s.join(" ")
}

// ----- HTTP handlers ---------------------------------------------------------

pub async fn protected_resource(State(app): State<AppState>, headers: HeaderMap) -> Json<Value> {
    let base = base_url(&app, &headers);
    Json(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["read", "write", "review"],
        "resource_name": "AiNKI",
    }))
}

pub async fn authorization_server(State(app): State<AppState>, headers: HeaderMap) -> Json<Value> {
    let base = base_url(&app, &headers);
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "revocation_endpoint": format!("{base}/oauth/revoke"),
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_post", "client_secret_basic"],
        "revocation_endpoint_auth_methods_supported": ["none", "client_secret_post", "client_secret_basic"],
        "scopes_supported": ["read", "write", "review"],
        "client_id_metadata_document_supported": true,
        "authorization_response_iss_parameter_supported": true,
    }))
}

pub async fn register(
    State(app): State<AppState>,
    crate::security::ClientIp(ip): crate::security::ClientIp,
    Json(req): Json<RegistrationRequest>,
) -> Response {
    // Registration is open by design (MCP clients register themselves), so
    // bound how fast one address can fill the table.
    if app.limiter.check_and_hit(&format!("oauth-register:{ip}"), 30, std::time::Duration::from_secs(3600)) {
        return crate::security::too_many_requests(3600);
    }
    match app.register_client(req).await {
        Ok(v) => (StatusCode::CREATED, [(header::CACHE_CONTROL, "no-store")], Json(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuthorizeParams {
    pub response_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub state: Option<String>,
    pub scope: Option<String>,
    pub resource: Option<String>,
}

/// Problems that must not be sent to the redirect URI (it may be untrusted).
fn authorize_error_page(user: Option<&crate::auth::User>, msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        layout(
            "Can't connect",
            user,
            html! {
                h1 { "Can't connect" }
                p.notice { (msg) }
                p { "Try removing and adding the connector again." }
            },
        ),
    )
        .into_response()
}

fn redirect_with(redirect_uri: &str, params: &[(&str, &str)]) -> Response {
    let mut url = redirect_uri.to_string();
    for (i, (k, v)) in params.iter().enumerate() {
        url.push(if i == 0 && !redirect_uri.contains('?') { '?' } else { '&' });
        url.push_str(k);
        url.push('=');
        url.push_str(&urlenc(v));
    }
    Redirect::to(&url).into_response()
}

/// Validate the request; returns the client and redirect URI, or the
/// response to send instead.
/// RFC 8707: a client may name the resource it wants a token for. Only this
/// server's MCP endpoint (or the server itself) is acceptable.
fn resource_ok(resource: Option<&str>, base: &str) -> bool {
    match resource {
        None => true,
        Some(r) => {
            let r = r.trim_end_matches('/');
            r == base || r == format!("{base}/mcp")
        }
    }
}

async fn check_authorize(
    app: &App,
    p: &AuthorizeParams,
    user: Option<&crate::auth::User>,
    base: &str,
) -> Result<(Client, String), Response> {
    let Some(client_id) = p.client_id.as_deref() else {
        return Err(authorize_error_page(user, "The request is missing client_id."));
    };
    let client = match app.client(client_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return Err(authorize_error_page(user, "This app isn't registered with AiNKI.")),
        Err(e) => return Err(crate::web::WebError::from(e).into_response()),
    };
    let redirects = client.redirects();
    let redirect_uri = match p.redirect_uri.as_deref() {
        Some(r) if client.allows_redirect(r) => r.to_string(),
        None if redirects.len() == 1 => redirects[0].clone(),
        _ => return Err(authorize_error_page(user, "The app's redirect address doesn't match its registration.")),
    };
    let fail = |error: &str, desc: &str| {
        let mut params = vec![("error", error), ("error_description", desc)];
        if let Some(s) = p.state.as_deref() {
            params.push(("state", s));
        }
        Err(redirect_with(&redirect_uri, &params))
    };
    if p.response_type.as_deref() != Some("code") {
        return fail("unsupported_response_type", "only response_type=code is supported");
    }
    let challenge_ok = p.code_challenge.as_deref().is_some_and(|c| {
        (43..=128).contains(&c.len()) && c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    });
    if !challenge_ok || p.code_challenge_method.as_deref() != Some("S256") {
        return fail("invalid_request", "PKCE with S256 is required");
    }
    if p.state.as_deref().is_some_and(|st| st.len() > 2048) {
        return fail("invalid_request", "state is too long");
    }
    if !resource_ok(p.resource.as_deref(), base) {
        return fail("invalid_target", "unknown resource");
    }
    Ok((client, redirect_uri))
}

pub async fn authorize_page(
    State(app): State<AppState>,
    headers: HeaderMap,
    Query(p): Query<AuthorizeParams>,
    user: Option<WebUser>,
) -> Response {
    let Some(WebUser(user)) = user else {
        // Log in first, then come back here.
        return Redirect::to(&format!("/login?next={}", urlenc(&authorize_path(&p)))).into_response();
    };
    let (client, redirect_uri) = match check_authorize(&app, &p, Some(&user), &base_url(&app, &headers)).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let access = crate::auth::Access::web(user.clone());
    let decks = app.decks(&access).await.unwrap_or_default();
    let target = redirect_uri
        .split("://")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or(&redirect_uri)
        .to_string();
    let hidden = |name: &str, v: &Option<String>| {
        html! { @if let Some(v) = v { input type="hidden" name=(name) value=(v); } }
    };
    layout(
        "Connect",
        Some(&user),
        html! {
            h1 { "Connect an app" }
            p { "An app calling itself " strong { (client.name) } " wants to use your AiNKI account " strong { (user.handle()) } "." }
            p.notice {
                "After you allow it, you'll be sent to " strong { (target) } ". "
                "Only continue if you started connecting this app yourself and that address is the one you expect "
                "(for Claude: claude.ai)."
            }
            form method="post" action="/oauth/authorize" {
                (hidden("response_type", &p.response_type))
                (hidden("client_id", &p.client_id))
                input type="hidden" name="redirect_uri" value=(redirect_uri);
                (hidden("code_challenge", &p.code_challenge))
                (hidden("code_challenge_method", &p.code_challenge_method))
                (hidden("state", &p.state))
                (hidden("scope", &p.scope))
                (hidden("resource", &p.resource))
                p { "It can always read your decks and cards. Also allow it to:" }
                label.check { input type="checkbox" name="can_write" value="1" checked; " Add, edit and delete cards and decks" }
                label.check { input type="checkbox" name="can_review" value="1" checked; " Quiz you and record your answers" }
                fieldset {
                    legend { "Decks" }
                    label.check { input type="radio" name="scope_decks" value="all" checked; " All decks" }
                    @if !decks.is_empty() {
                        label.check { input type="radio" name="scope_decks" value="some"; " Only these:" }
                        @for d in &decks {
                            label.check.indent { input type="checkbox" name="decks" value=(d.id); " " (d.name) }
                        }
                    }
                }
                div.actions {
                    button.btn.primary type="submit" name="decision" value="allow" { "Allow" }
                    button.btn type="submit" name="decision" value="deny" { "Cancel" }
                }
            }
            p.muted.small { "You can disconnect it any time under Settings → Agent access." }
        },
    )
    .into_response()
}

fn authorize_path(p: &AuthorizeParams) -> String {
    let mut q = Vec::new();
    for (k, v) in [
        ("response_type", &p.response_type),
        ("client_id", &p.client_id),
        ("redirect_uri", &p.redirect_uri),
        ("code_challenge", &p.code_challenge),
        ("code_challenge_method", &p.code_challenge_method),
        ("state", &p.state),
        ("scope", &p.scope),
        ("resource", &p.resource),
    ] {
        if let Some(v) = v {
            q.push(format!("{k}={}", urlenc(v)));
        }
    }
    format!("/oauth/authorize?{}", q.join("&"))
}

#[derive(Debug, Deserialize)]
pub struct ConsentForm {
    #[serde(flatten)]
    pub params: AuthorizeParams,
    pub decision: String,
    pub can_write: Option<String>,
    pub can_review: Option<String>,
    pub scope_decks: Option<String>,
    #[serde(default)]
    pub decks: Vec<i64>,
}

pub async fn authorize_submit(State(app): State<AppState>, headers: HeaderMap, WebUser(user): WebUser, Form(f): Form<ConsentForm>) -> Response {
    let (client, redirect_uri) = match check_authorize(&app, &f.params, Some(&user), &base_url(&app, &headers)).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let iss = base_url(&app, &headers);
    let mut params: Vec<(&str, &str)> = Vec::new();
    if f.decision != "allow" {
        params.push(("error", "access_denied"));
        params.push(("error_description", "the user declined"));
    }
    let decks = if f.scope_decks.as_deref() == Some("some") { Some(f.decks.clone()) } else { None };
    if decks.as_ref().is_some_and(|d| d.is_empty()) {
        return authorize_error_page(Some(&user), "Pick at least one deck, or allow all decks.");
    }
    let code;
    if f.decision == "allow" {
        code = match app
            .create_auth_code(
                user.id,
                &client,
                &redirect_uri,
                f.params.code_challenge.as_deref().unwrap_or(""),
                f.can_write.is_some(),
                f.can_review.is_some(),
                decks,
            )
            .await
        {
            Ok(c) => c,
            Err(e) => return crate::web::WebError::from(e).into_response(),
        };
        params.push(("code", &code));
    }
    if let Some(s) = f.params.state.as_deref() {
        params.push(("state", s));
    }
    params.push(("iss", &iss));
    redirect_with(&redirect_uri, &params)
}

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_verifier: Option<String>,
    pub refresh_token: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub resource: Option<String>,
}

/// Client credentials from HTTP Basic auth, if present.
fn basic_auth(headers: &HeaderMap) -> Option<(String, String)> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let b64 = v.strip_prefix("Basic ").or_else(|| v.strip_prefix("basic "))?;
    let raw = base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?;
    let s = String::from_utf8(raw).ok()?;
    let (id, secret) = s.split_once(':')?;
    // RFC 6749 2.3.1: both parts are form-urlencoded.
    let dec = |x: &str| -> String {
        let x = x.replace('+', " ");
        let bytes = x.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(b);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    Some((dec(id), dec(secret)))
}

pub async fn token(State(app): State<AppState>, headers: HeaderMap, Form(t): Form<TokenRequest>) -> Response {
    if !resource_ok(t.resource.as_deref(), &base_url(&app, &headers)) {
        return OAuthError::new("invalid_target", "unknown resource").into_response();
    }
    let (client_id, secret) = match basic_auth(&headers) {
        Some((id, secret)) => (Some(id), Some(secret)),
        None => (t.client_id.clone(), t.client_secret.clone()),
    };
    let Some(client_id) = client_id else {
        return OAuthError::new("invalid_client", "client_id is required").into_response();
    };
    let result = match t.grant_type.as_str() {
        "authorization_code" => match (t.code.as_deref(), t.code_verifier.as_deref()) {
            (Some(code), Some(verifier)) => {
                app.exchange_code(&client_id, secret.as_deref(), code, t.redirect_uri.as_deref(), verifier).await
            }
            _ => Err(OAuthError::new("invalid_request", "code and code_verifier are required")),
        },
        "refresh_token" => match t.refresh_token.as_deref() {
            Some(rt) => app.refresh_tokens(&client_id, secret.as_deref(), rt).await,
            None => Err(OAuthError::new("invalid_request", "refresh_token is required")),
        },
        _ => Err(OAuthError::new("unsupported_grant_type", "use authorization_code or refresh_token")),
    };
    match result {
        Ok(tokens) => ([(header::CACHE_CONTROL, "no-store"), (header::PRAGMA, "no-cache")], Json(tokens)).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct RevokeRequest {
    pub token: String,
}

pub async fn revoke(State(app): State<AppState>, Form(r): Form<RevokeRequest>) -> Response {
    match app.revoke_oauth_token(&r.token).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => OAuthError::from(e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_rules() {
        assert!(valid_redirect("https://claude.ai/api/mcp/auth_callback"));
        assert!(valid_redirect("http://localhost:33418/callback"));
        assert!(valid_redirect("http://127.0.0.1/cb"));
        assert!(!valid_redirect("http://evil.example/cb"));
        assert!(!valid_redirect("javascript:alert(1)"));
        assert!(!valid_redirect("https://a.example/cb#frag"));
        assert!(same_loopback("http://localhost:1234/cb", "http://localhost:9999/cb"));
        assert!(!same_loopback("http://localhost:1234/cb", "http://localhost:9999/other"));
    }

    #[test]
    fn only_public_addresses_are_fetched() {
        for ip in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1", "64:ff9b::a00:1"] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "160.79.104.10", "2606:4700::1111"] {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[tokio::test]
    async fn metadata_urls_must_be_safe() {
        for url in ["http://example.com/client", "https://user@example.com/c", "https://example.com:8443/c", "https://example.com/", "https://127.0.0.1/c", "https://localhost/c"] {
            assert!(fetch_metadata_document(url).await.is_none(), "{url}");
        }
    }

    #[test]
    fn resources() {
        let base = "https://cards.example.com";
        assert!(resource_ok(None, base));
        assert!(resource_ok(Some("https://cards.example.com/mcp"), base));
        assert!(resource_ok(Some("https://cards.example.com/"), base));
        assert!(!resource_ok(Some("https://evil.example/mcp"), base));
        assert!(ct_eq(b"abc", b"abc") && !ct_eq(b"abc", b"abd") && !ct_eq(b"ab", b"abc"));
    }

    #[test]
    fn pkce() {
        // RFC 7636 appendix B
        assert!(pkce_ok(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        ));
        assert!(!pkce_ok("short", "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"));
    }

    #[test]
    fn basic_auth_decoding() {
        let mut h = HeaderMap::new();
        let v = base64::engine::general_purpose::STANDARD.encode("my%20id:s3cr%2Bt");
        h.insert(header::AUTHORIZATION, format!("Basic {v}").parse().unwrap());
        assert_eq!(basic_auth(&h), Some(("my id".into(), "s3cr+t".into())));
    }
}
