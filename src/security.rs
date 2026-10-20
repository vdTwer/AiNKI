//! Cross-cutting protections: rate limiting, client IPs behind a reverse
//! proxy, security headers, Origin checks for cookie-authenticated requests,
//! and flash messages (kept in a cookie instead of the URL so nobody can
//! put arbitrary text on an AiNKI page with a crafted link).

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::AppState;

pub const FLASH_COOKIE: &str = "ainki_flash";

/// Fixed-window counters keyed by strings like "login-ip:1.2.3.4".
#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, (u32, Instant)>>,
}

impl RateLimiter {
    fn with_bucket<T>(&self, key: &str, window: Duration, f: impl FnOnce(&mut (u32, Instant)) -> T) -> T {
        let mut map = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() > 50_000 {
            map.retain(|_, (_, start)| start.elapsed() < Duration::from_secs(3600));
        }
        let entry = map.entry(key.to_string()).or_insert((0, Instant::now()));
        if entry.1.elapsed() >= window {
            *entry = (0, Instant::now());
        }
        f(entry)
    }

    /// True if `key` has already used up `max` hits in the window.
    pub fn exceeded(&self, key: &str, max: u32, window: Duration) -> bool {
        self.with_bucket(key, window, |(n, _)| *n >= max)
    }

    pub fn hit(&self, key: &str, window: Duration) {
        self.with_bucket(key, window, |(n, _)| *n += 1);
    }

    /// Count a hit and report whether the limit is now exceeded.
    pub fn check_and_hit(&self, key: &str, max: u32, window: Duration) -> bool {
        self.with_bucket(key, window, |(n, _)| {
            *n += 1;
            *n > max
        })
    }

    pub fn reset(&self, key: &str) {
        self.buckets.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
    }
}

fn is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback() || v4.is_private())
        }
    }
}

/// The client's IP. Behind a reverse proxy (Caddy) on the same host or
/// Docker network the direct peer is the proxy, so the last
/// X-Forwarded-For entry (the one the proxy added) is used. From any other
/// peer the header is ignored, since anyone could set it.
pub fn client_ip(headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
    let forwarded = || {
        headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .last()
            .and_then(|s| s.trim().parse::<IpAddr>().ok())
    };
    match peer {
        Some(p) if is_internal(p) => forwarded().or(Some(p)),
        Some(p) => Some(p),
        None => forwarded(),
    }
}

/// Extractor for the client IP.
pub struct ClientIp(pub String);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
        Ok(ClientIp(
            client_ip(&parts.headers, peer).map(|ip| ip.to_string()).unwrap_or_else(|| "unknown".into()),
        ))
    }
}

pub fn too_many_requests(retry_after_secs: u64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, retry_after_secs.to_string())],
        "Too many attempts. Please wait a while and try again.",
    )
        .into_response()
}

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
img-src 'self'; connect-src 'self'; worker-src 'self'; object-src 'none'; base-uri 'none'; \
frame-ancestors 'none'";

/// Security headers on every response. HTML pages that don't set their own
/// caching are marked no-store so private pages don't linger in caches.
pub async fn headers(State(app): State<AppState>, req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert("content-security-policy", HeaderValue::from_static(CSP));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    h.insert("cross-origin-opener-policy", HeaderValue::from_static("same-origin"));
    h.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
    );
    if app.config.secure() {
        h.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    if !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    Some(format!("{}://{}", scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

fn host_of(origin: &str) -> &str {
    origin.split_once("://").map(|(_, h)| h).unwrap_or(origin)
}

/// Hosts a legitimate same-origin request can come from: the Host the
/// browser connected to (Caddy passes it through) and the public URL.
fn allowed_hosts(app: &AppState, headers: &HeaderMap) -> Vec<String> {
    let mut hosts = Vec::new();
    if let Some(h) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) {
        hosts.push(h.to_ascii_lowercase());
    }
    if let Some(u) = &app.config.public_url {
        hosts.push(host_of(u).to_ascii_lowercase());
    }
    hosts
}

/// Reject cross-origin state-changing requests to cookie-authenticated
/// routes. SameSite=Lax cookies already stop other sites; this also stops
/// sibling subdomains (which count as "same site").
pub async fn check_origin(State(app): State<AppState>, req: Request, next: Next) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(req).await;
    }
    let headers = req.headers();
    let claimed = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|o| if o == "null" { Some("null".to_string()) } else { origin_of(o) })
        .or_else(|| headers.get(header::REFERER).and_then(|v| v.to_str().ok()).map(origin_of));
    if let Some(claimed) = claimed {
        let ok = claimed
            .as_deref()
            .is_some_and(|c| c != "null" && allowed_hosts(&app, headers).iter().any(|h| h == host_of(c)));
        if !ok {
            tracing::warn!("blocked cross-origin {} {} from {:?}", req.method(), req.uri().path(), claimed);
            return (StatusCode::FORBIDDEN, "Cross-origin request blocked.").into_response();
        }
    }
    next.run(req).await
}

/// The flash message for this request, if any.
#[derive(Clone, Default)]
pub struct Flash(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for Flash {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.extensions.get::<Flash>().cloned().unwrap_or_default())
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Moves the flash cookie into a request extension, and clears it once a
/// page (not a redirect) has been shown.
pub async fn flash(mut req: Request, next: Next) -> Response {
    let msg = req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().strip_prefix(&format!("{FLASH_COOKIE}=")).map(str::to_string))
        .next()
        .map(|v| percent_decode(&v))
        .filter(|m| !m.is_empty());
    let had = msg.is_some();
    req.extensions_mut().insert(Flash(msg.map(|m| m.chars().take(500).collect())));
    let mut res = next.run(req).await;
    if had && !res.status().is_redirection() && !res.headers().contains_key(header::SET_COOKIE) {
        res.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&format!("{FLASH_COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax"))
                .expect("static header"),
        );
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_for_only_from_internal_peers() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "6.6.6.6, 203.0.113.9".parse().unwrap());
        let local: IpAddr = "127.0.0.1".parse().unwrap();
        let docker: IpAddr = "172.18.0.3".parse().unwrap();
        let public: IpAddr = "198.51.100.7".parse().unwrap();
        assert_eq!(client_ip(&h, Some(local)), Some("203.0.113.9".parse().unwrap()));
        assert_eq!(client_ip(&h, Some(docker)), Some("203.0.113.9".parse().unwrap()));
        assert_eq!(client_ip(&h, Some(public)), Some(public));
        assert_eq!(client_ip(&HeaderMap::new(), Some(local)), Some(local));
    }

    #[test]
    fn limiter() {
        let l = RateLimiter::default();
        let w = Duration::from_secs(60);
        for _ in 0..3 {
            assert!(!l.check_and_hit("k", 3, w));
        }
        assert!(l.check_and_hit("k", 3, w));
        assert!(l.exceeded("k", 3, w));
        l.reset("k");
        assert!(!l.exceeded("k", 3, w));
    }

    #[test]
    fn origins() {
        assert_eq!(origin_of("https://Cards.example.com/x?y"), Some("https://cards.example.com".into()));
        assert_eq!(origin_of("http://localhost:8080"), Some("http://localhost:8080".into()));
        assert_eq!(percent_decode("Deck%20deleted%20(3%20cards)."), "Deck deleted (3 cards).");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
