//! Sign-in with user accounts (see `users.rs`).
//!
//! - No accounts: the hub listens on a loopback address only (the server refuses anything
//!   else) and asks for nothing, as before. `/api/auth/status` reports `setup: true` and the
//!   first account can be created from this machine (`POST /api/auth/setup`) or with
//!   `hubness user add <name>`.
//! - With accounts every `/api` request and every preview port needs a session:
//!   - API clients (the TUI, scripts) send `Authorization: Bearer <token>` from
//!     `POST /api/auth/login`;
//!   - browsers get the same session as an HttpOnly cookie (EventSource can't send headers).
//! - The hub's own helper processes (the MCP server calls the hub back) use a random
//!   per-process service token passed in their environment; it is never shown to anyone.
//!
//! The hub doesn't do TLS: across untrusted networks put it behind an HTTPS reverse proxy or
//! a VPN.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json};
use hubness_core::{AuthStatus, LoginResponse, User};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::error::{ApiResult, HttpError};
use crate::http::JsonBody;
use crate::users;

pub const COOKIE: &str = "hubness_session";
/// The cookie of the agents' headless browser on a preview port (see `preview_pass`).
pub const PREVIEW_COOKIE: &str = "hubness_preview";
const COOKIE_MAX_AGE: u64 = 30 * 24 * 3600;

static DB: OnceLock<Arc<Mutex<Connection>>> = OnceLock::new();
static SERVICE: OnceLock<String> = OnceLock::new();

/// Turn sign-in on for this process: the database with the accounts and a fresh service
/// token for the hub's own children. Until it is called (tests that build the router
/// directly) nothing is checked.
pub fn init(db: Arc<Mutex<Connection>>) -> std::io::Result<()> {
    let _ = SERVICE.set(users::random_token()?);
    let _ = DB.set(db);
    Ok(())
}

/// The token for the hub's own helper processes (passed only in their environment).
pub fn service_token() -> Option<String> {
    SERVICE.get().cloned()
}

/// Who makes a request.
#[derive(Clone, Debug)]
pub enum Principal {
    /// No accounts yet and the request comes from this machine.
    Open,
    /// One of the hub's own processes (service token).
    Service,
    /// A signed-in user and the session token used.
    User { user: User, token: String },
}

/// What the guard learned about a request, for the handlers.
#[derive(Clone, Debug)]
pub struct Caller {
    pub principal: Option<Principal>,
    /// The peer is this machine and no proxy forwarded the request.
    pub local: bool,
    pub ip: Option<IpAddr>,
}

impl Caller {
    fn user(&self) -> Option<&User> {
        match &self.principal {
            Some(Principal::User { user, .. }) => Some(user),
            _ => None,
        }
    }

    fn token(&self) -> Option<&str> {
        match &self.principal {
            Some(Principal::User { token, .. }) => Some(token),
            _ => None,
        }
    }
}

fn db() -> Option<std::sync::MutexGuard<'static, Connection>> {
    DB.get().map(|d| d.lock().unwrap_or_else(|e| e.into_inner()))
}

fn eq(a: &str, b: &str) -> bool {
    // constant time for equal lengths; the length of a token is not a secret
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::trim)
}

/// The principal behind one token.
fn principal_for(db: &Connection, token: &str) -> Option<Principal> {
    if token.is_empty() {
        return None;
    }
    if SERVICE.get().is_some_and(|s| eq(s, token)) {
        return Some(Principal::Service);
    }
    users::session_user(db, token).ok().flatten().map(|user| Principal::User { user, token: token.to_string() })
}

/// The principal behind the request's credentials (bearer header first, then the cookie).
fn authenticate(db: &Connection, headers: &HeaderMap) -> Option<Principal> {
    [bearer(headers), cookie_value(headers, COOKIE)].into_iter().flatten().find_map(|t| principal_for(db, t))
}

fn has_users(db: &Connection) -> bool {
    users::count(db).map(|n| n > 0).unwrap_or(true)
}

/// Is the peer this machine (and not a local reverse proxy relaying someone else)?
fn local_peer(req: &Request) -> (bool, Option<IpAddr>) {
    let ip = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
    let forwarded = ["x-forwarded-for", "forwarded", "x-real-ip"].iter().any(|h| req.headers().contains_key(*h));
    (ip.is_some_and(|ip| ip.is_loopback()) && !forwarded, ip)
}

/// What lets the agents' headless browser through the preview proxy of one port on a hub with
/// accounts. It is not a hub credential: `/api` never reads this cookie, and the value opens
/// nothing but that one preview port — so a previewed app that gets to see it (cookies are
/// shared by every port of a host) learns nothing it can use against the hub.
pub fn preview_pass(port: u16) -> String {
    use sha2::{Digest, Sha256};
    static SECRET: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| users::random_token().unwrap_or_else(|_| uuid::Uuid::new_v4().to_string()));
    Sha256::digest(format!("{}:{port}", *SECRET)).iter().map(|b| format!("{b:02x}")).collect()
}

fn preview_authorized(db: &Connection, headers: &HeaderMap, port: u16) -> bool {
    !has_users(db)
        || authenticate(db, headers).is_some()
        || cookie_value(headers, PREVIEW_COOKIE).is_some_and(|v| eq(v, &preview_pass(port)))
}

/// May the request use the preview of `port`? Preview ports listen on the hub's own address:
/// the browser's hub session (or no accounts at all), or the agents' browser with its pass.
pub fn authorized(headers: &HeaderMap, port: u16) -> bool {
    let Some(db) = db() else { return true };
    preview_authorized(&db, headers, port)
}

/// Drop the hub's own credentials (and the preview pass) before a request goes to a
/// previewed app.
pub fn strip_credentials(headers: &mut HeaderMap) {
    if bearer(headers).is_some_and(|t| db().is_some_and(|db| principal_for(&db, t).is_some())) {
        headers.remove(header::AUTHORIZATION);
    }
    let rest: Vec<String> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .filter(|kv| !kv.is_empty() && !kv.starts_with(&format!("{COOKIE}=")) && !kv.starts_with(&format!("{PREVIEW_COOKIE}=")))
        .map(String::from)
        .collect();
    headers.remove(header::COOKIE);
    if !rest.is_empty()
        && let Ok(v) = HeaderValue::from_str(&rest.join("; "))
    {
        headers.insert(header::COOKIE, v);
    }
}

/// `/api` guard: everything but health and the auth routes needs a session once the hub has
/// accounts. The handlers get a `Caller`.
pub async fn guard(mut req: Request, next: Next) -> Response {
    let (local, ip) = local_peer(&req);
    // `None` until `init` (tests building the router directly): nothing is checked
    let checked = db().map(|db| (authenticate(&db, req.headers()), has_users(&db)));
    let Some((mut principal, users_exist)) = checked else {
        req.extensions_mut().insert(Caller { principal: Some(Principal::Open), local, ip });
        return next.run(req).await;
    };
    if principal.is_none() && !users_exist && local {
        principal = Some(Principal::Open);
    }
    let path = req.uri().path();
    let open = path == "/health" || path.starts_with("/auth/") || path == "/api/health" || path.starts_with("/api/auth/");
    if !open && principal.is_none() {
        let msg = if users_exist {
            "Sign in to hubness"
        } else {
            "This hub has no accounts yet: create one on its machine with `hubness user add <name>`"
        };
        return HttpError::new(401, msg).into_response();
    }
    req.extensions_mut().insert(Caller { principal, local, ip });
    next.run(req).await
}

pub fn routes() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/auth/status", get(status))
        .route("/auth/login", post(login))
        .route("/auth/setup", post(setup))
        .route("/auth/logout", post(logout))
        .route("/auth/password", put(change_password))
}

fn with_db<T>(f: impl FnOnce(&Connection) -> ApiResult<T>) -> ApiResult<T> {
    match db() {
        Some(db) => f(&db),
        None => Err(HttpError::new(503, "Accounts are not available on this server")),
    }
}

/// Hash a password off the async threads.
async fn hash(password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || users::hash_password(&password)).await.map_err(|e| HttpError::new(500, e.to_string()))?
}

async fn verify(password: String, hash: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || users::verify_password(&password, hash.as_deref())).await.unwrap_or(false)
}

async fn status(Extension(c): Extension<Caller>) -> Json<AuthStatus> {
    let required = db().is_some_and(|db| has_users(&db));
    Json(AuthStatus { required, setup: !required, user: c.user().cloned() })
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

fn cookie(headers: &HeaderMap, value: &str, max_age: u64) -> String {
    // Secure when the hub is reached through an HTTPS proxy
    let https = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).is_some_and(|p| p.eq_ignore_ascii_case("https"));
    format!("{COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}", if https { "; Secure" } else { "" })
}

/// A new session: the token in the body for API clients, the cookie for browsers.
fn signed_in(headers: &HeaderMap, user: User) -> ApiResult<Response> {
    let token = with_db(|db| users::create_session(db, &user.id))?;
    let set = cookie(headers, &token, COOKIE_MAX_AGE);
    Ok((StatusCode::OK, [(header::SET_COOKIE, set)], Json(LoginResponse { token, user })).into_response())
}

// ----- failed sign-ins: a growing pause per username + address -----

const FREE_ATTEMPTS: u32 = 5;
const MAX_LOCK: Duration = Duration::from_secs(15 * 60);

static FAILURES: Mutex<Option<HashMap<String, (u32, Instant)>>> = Mutex::new(None);

fn limit_key(username: &str, ip: Option<IpAddr>) -> String {
    format!("{}|{}", username.trim().to_lowercase(), ip.map(|i| i.to_string()).unwrap_or_default())
}

/// Seconds to wait before the next attempt is accepted.
fn locked_for(key: &str) -> Option<u64> {
    let map = FAILURES.lock().unwrap_or_else(|e| e.into_inner());
    let (n, until) = map.as_ref()?.get(key)?;
    let now = Instant::now();
    (*n >= FREE_ATTEMPTS && *until > now).then(|| (*until - now).as_secs().max(1))
}

fn record_failure(key: &str) {
    let mut map = FAILURES.lock().unwrap_or_else(|e| e.into_inner());
    let map = map.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    if map.len() > 10_000 {
        map.retain(|_, (_, until)| *until + MAX_LOCK > now);
    }
    let e = map.entry(key.to_string()).or_insert((0, now));
    e.0 += 1;
    if e.0 >= FREE_ATTEMPTS {
        // 30 s after the fifth failure, doubling up to 15 minutes
        let lock = Duration::from_secs(30u64.saturating_mul(1 << (e.0 - FREE_ATTEMPTS).min(10))).min(MAX_LOCK);
        e.1 = now + lock;
    } else {
        e.1 = now;
    }
}

fn clear_failures(key: &str) {
    if let Some(map) = FAILURES.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        map.remove(key);
    }
}

async fn login(headers: HeaderMap, Extension(c): Extension<Caller>, JsonBody(b): JsonBody<Credentials>) -> ApiResult<Response> {
    let key = limit_key(&b.username, c.ip);
    if let Some(secs) = locked_for(&key) {
        return Err(HttpError::new(429, format!("Too many failed sign-ins: try again in {secs} s")));
    }
    let found = with_db(|db| Ok(users::find(db, &b.username)?))?;
    let (user, hash) = found.map(|(u, h)| (Some(u), Some(h))).unwrap_or((None, None));
    if !verify(b.password, hash).await {
        record_failure(&key);
        return Err(HttpError::new(401, "Wrong username or password"));
    }
    clear_failures(&key);
    let Some(user) = user else { return Err(HttpError::new(401, "Wrong username or password")) };
    signed_in(&headers, user)
}

/// The first account, from this machine only (anyone else could otherwise claim the hub).
async fn setup(headers: HeaderMap, Extension(c): Extension<Caller>, JsonBody(b): JsonBody<Credentials>) -> ApiResult<Response> {
    if with_db(|db| Ok(has_users(db)))? {
        return Err(HttpError::new(409, "The hub already has accounts: sign in"));
    }
    if !c.local {
        return Err(HttpError::new(403, "The first account can only be created on the hub's machine (or with `hubness user add <name>`)"));
    }
    let username = users::check_username(&b.username)?;
    users::check_password(&b.password)?;
    let hash = hash(b.password).await?;
    let user = with_db(|db| users::create_first(db, &username, &hash))?;
    signed_in(&headers, user)
}

async fn logout(headers: HeaderMap, Extension(c): Extension<Caller>) -> Response {
    if let Some(token) = c.token() {
        let _ = with_db(|db| Ok(users::delete_session(db, token)?));
    }
    (StatusCode::OK, [(header::SET_COOKIE, cookie(&headers, "", 0))], Json(json!({ "ok": true }))).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PasswordBody {
    current_password: String,
    password: String,
}

/// A new password for the hub's account (the current one is required). Ends its other
/// sessions; the one that changed it stays signed in.
async fn change_password(Extension(c): Extension<Caller>, JsonBody(b): JsonBody<PasswordBody>) -> ApiResult<Json<serde_json::Value>> {
    let Some(user) = c.user().cloned() else {
        // /auth/* is open: without a session it's a sign-in that is missing — unless there is no account
        return Err(if with_db(|db| Ok(has_users(db)))? {
            HttpError::new(401, "Sign in first")
        } else {
            HttpError::new(409, "The hub has no account: create it first")
        });
    };
    users::check_password(&b.password)?;
    let Some(stored) = with_db(|db| Ok(users::password_hash(db, &user.id)?))? else { return Err(HttpError::not_found("No such account")) };
    // 403, not 401: a wrong current password must not look like a lost session
    if !verify(b.current_password, Some(stored)).await {
        return Err(HttpError::new(403, "The current password is wrong"));
    }
    let hash = hash(b.password).await?;
    with_db(|db| users::set_password(db, &user.id, &hash, c.token()))?;
    Ok(Json(json!({ "ok": true })))
}

/// Is the address a loopback one (127.0.0.0/8, ::1, localhost)?
pub fn is_loopback(host: &str) -> bool {
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
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
        assert!(cookie(&h, "t", 10).contains("HttpOnly; SameSite=Lax"));
    }

    #[test]
    fn the_preview_pass_opens_one_preview_port_and_never_the_api() {
        let db = Connection::open_in_memory().unwrap();
        crate::db::init(&db).unwrap();
        let cookie = |v: String| {
            let mut h = HeaderMap::new();
            h.insert(header::COOKIE, HeaderValue::from_str(&v).unwrap());
            h
        };
        let pass = preview_pass(8790);
        assert_eq!(pass.len(), 64);
        assert_eq!(pass, preview_pass(8790));
        assert_ne!(pass, preview_pass(8791), "a pass is for one port");
        assert!(preview_authorized(&db, &HeaderMap::new(), 8790), "no account: previews are as open as the hub");

        let user = users::create_first(&db, "alice", "$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$aGFzaA").unwrap();
        let session = users::create_session(&db, &user.id).unwrap();
        assert!(!preview_authorized(&db, &HeaderMap::new(), 8790));
        assert!(preview_authorized(&db, &cookie(format!("{PREVIEW_COOKIE}={pass}")), 8790), "the agents' browser");
        assert!(!preview_authorized(&db, &cookie(format!("{PREVIEW_COOKIE}={pass}")), 8791), "not another preview");
        assert!(!preview_authorized(&db, &cookie(format!("{PREVIEW_COOKIE}=nope")), 8790));
        assert!(preview_authorized(&db, &cookie(format!("{COOKIE}={session}")), 8790), "the user's own browser");

        // the API knows nothing of the pass, under either cookie name or as a bearer token
        assert!(authenticate(&db, &cookie(format!("{PREVIEW_COOKIE}={pass}"))).is_none());
        assert!(authenticate(&db, &cookie(format!("{COOKIE}={pass}"))).is_none());
        assert!(authenticate(&db, &cookie(format!("{PREVIEW_COOKIE}={session}"))).is_none(), "nor does it read that cookie at all");
        let mut bearer = HeaderMap::new();
        bearer.insert(header::AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {pass}")).unwrap());
        assert!(authenticate(&db, &bearer).is_none());

        // nothing of the hub's goes on to the previewed app
        let mut h = cookie(format!("a=1; {COOKIE}={session}; {PREVIEW_COOKIE}={pass}; b=2"));
        strip_credentials(&mut h);
        assert_eq!(h.get(header::COOKIE).unwrap(), "a=1; b=2");
        let mut only = cookie(format!("{PREVIEW_COOKIE}={pass}"));
        strip_credentials(&mut only);
        assert!(only.get(header::COOKIE).is_none());
    }

    #[test]
    fn failed_sign_ins_back_off() {
        let key = limit_key(" Someone ", None);
        assert_eq!(key, "someone|");
        for _ in 0..FREE_ATTEMPTS - 1 {
            record_failure(&key);
        }
        assert_eq!(locked_for(&key), None, "a few typos are free");
        record_failure(&key);
        assert!(locked_for(&key).is_some_and(|s| s <= 30));
        clear_failures(&key);
        assert_eq!(locked_for(&key), None);
    }
}
