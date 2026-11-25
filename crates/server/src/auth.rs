//! Access token for a hub reachable from other machines.
//!
//! Off by default: without `HUBNESS_TOKEN` the hub only listens on 127.0.0.1 and asks for
//! nothing (as before). With a token every `/api` request and every preview port needs it:
//! - API clients (the TUI, scripts) send `Authorization: Bearer <token>`;
//! - the browser signs in once (`POST /api/auth/login`) and gets an HttpOnly cookie, because
//!   EventSource can't send headers. The cookie holds a hash of the token, not the token.
//!
//! The hub refuses to listen on a non-loopback address without a token. It doesn't do TLS:
//! over an untrusted network put it behind an HTTPS reverse proxy or a VPN.

use std::sync::OnceLock;
use std::time::Duration;

use axum::Json;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::error::HttpError;

pub const COOKIE: &str = "hubness_session";
const MIN_LEN: usize = 16;

static TOKEN: OnceLock<Option<String>> = OnceLock::new();

/// Set the token once at startup (`None` — no authentication).
pub fn init(token: Option<String>) -> anyhow::Result<()> {
    if let Some(t) = &token
        && t.chars().count() < MIN_LEN
    {
        anyhow::bail!("HUBNESS_TOKEN must be at least {MIN_LEN} characters (generate one with `hubness token`)");
    }
    let _ = TOKEN.set(token);
    Ok(())
}

fn token() -> Option<&'static str> {
    TOKEN.get().and_then(|t| t.as_deref())
}

/// The token for the hub's own helper processes (the MCP server calls the hub back).
pub fn token_for_children() -> Option<String> {
    token().map(String::from)
}

pub fn required() -> bool {
    token().is_some()
}

/// A random token for `HUBNESS_TOKEN` (256 bits, URL-safe).
pub fn generate() -> std::io::Result<String> {
    use base64::Engine;
    let mut buf = [0u8; 32];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut buf)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

/// What the browser cookie holds: a hash of the token (the token itself never sits in a cookie).
fn session_value(token: &str) -> String {
    Sha256::digest(format!("hubness-session:{token}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn eq(a: &str, b: &str) -> bool {
    // constant time for equal lengths; the length of a token is not a secret worth hiding
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

/// Does the request carry the token (bearer header or the session cookie)?
pub fn authorized(headers: &HeaderMap) -> bool {
    let Some(t) = token() else { return true };
    if let Some(b) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "))
        && eq(b.trim(), t)
    {
        return true;
    }
    cookie_value(headers, COOKIE).is_some_and(|c| eq(c, &session_value(t)))
}

/// Drop the hub's own credentials before a request goes to a previewed app.
pub fn strip_credentials(headers: &mut HeaderMap) {
    let Some(t) = token() else { return };
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.strip_prefix("Bearer ").is_some_and(|b| eq(b.trim(), t)))
    {
        headers.remove(header::AUTHORIZATION);
    }
    let rest: Vec<String> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .filter(|kv| !kv.is_empty() && !kv.starts_with(&format!("{COOKIE}=")))
        .map(String::from)
        .collect();
    headers.remove(header::COOKIE);
    if !rest.is_empty()
        && let Ok(v) = HeaderValue::from_str(&rest.join("; "))
    {
        headers.insert(header::COOKIE, v);
    }
}

/// `/api` guard: everything but health and the auth routes needs the token.
pub async fn guard(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let open = path == "/health" || path.starts_with("/auth/") || path == "/api/health" || path.starts_with("/api/auth/");
    if !open && !authorized(req.headers()) {
        return HttpError::new(401, "Sign in to hubness: an access token is required").into_response();
    }
    next.run(req).await
}

pub fn routes() -> axum::Router<AppState> {
    axum::Router::new().route("/auth/status", get(status)).route("/auth/login", post(login)).route("/auth/logout", post(logout))
}

async fn status(headers: HeaderMap) -> Json<serde_json::Value> {
    Json(json!({ "required": required(), "authenticated": authorized(&headers) }))
}

#[derive(Deserialize)]
struct LoginBody {
    token: String,
}

fn cookie(headers: &HeaderMap, value: &str, max_age: u64) -> String {
    // Secure when the hub is reached through an HTTPS proxy
    let https = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).is_some_and(|p| p.eq_ignore_ascii_case("https"));
    format!("{COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{}", if https { "; Secure" } else { "" })
}

/// Exchange the token for the browser session cookie.
async fn login(headers: HeaderMap, crate::http::JsonBody(b): crate::http::JsonBody<LoginBody>) -> Response {
    let Some(t) = token() else { return Json(json!({ "ok": true })).into_response() };
    if !eq(b.token.trim(), t) {
        // slow down guessing
        tokio::time::sleep(Duration::from_millis(800)).await;
        return HttpError::new(401, "Wrong access token").into_response();
    }
    let set = cookie(&headers, &session_value(t), 30 * 24 * 3600);
    (StatusCode::OK, [(header::SET_COOKIE, set)], Json(json!({ "ok": true }))).into_response()
}

async fn logout(headers: HeaderMap) -> Response {
    (StatusCode::OK, [(header::SET_COOKIE, cookie(&headers, "", 0))], Json(json!({ "ok": true }))).into_response()
}

/// Is the address a loopback one (127.0.0.0/8, ::1, localhost)?
pub fn is_loopback(host: &str) -> bool {
    host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_and_loopback() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; hubness_session=abc; b=2"));
        assert_eq!(cookie_value(&h, COOKIE), Some("abc"));
        assert!(is_loopback("127.0.0.1") && is_loopback("::1") && is_loopback("localhost"));
        assert!(!is_loopback("0.0.0.0") && !is_loopback("192.168.1.5"));
        assert_eq!(session_value("x").len(), 64);
        assert!(generate().unwrap().len() >= 43);
    }
}
