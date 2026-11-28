//! Preview of a running app (like Webview in Replit) — a port of `preview.ts`.
//!
//! Each preview is a separate hub port (range `HUBNESS_PREVIEW_PORTS`, 8790–8809 by
//! default) on the same host as the hub: so it opens from a phone too, even if the app
//! listens only on 127.0.0.1 or lives in a container. Port = separate origin: the app's JS
//! can't read responses of the hub API (and the hub rejects mutating requests without
//! X-Hubness-Request/JSON). The path is not rewritten — the app works from the root (/),
//! assets and HMR (WebSocket) work as is. The Host header is replaced with localhost:port —
//! dev servers (Vite) check it.
//!
//! Once hub login exists, move previews to subdomains (hub cookies are shared across all
//! ports of one host).
//!
//! Pages (HTML) get a small helper script (`packages/shared/assets/preview-inject.js`,
//! served as `/__hubness/preview.js`): it reports console errors, uncaught errors and failed
//! requests to `/__hubness/log` (kept per preview, read by the Preview panel and the agent's
//! `preview_console`), tells the panel where the page is and runs the element picker.
//! `HUBNESS_PREVIEW_INJECT=0` turns it off.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use axum::body::{Body, Bytes};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use hubness_core::PreviewLog;
use hyper::body::Incoming;
use hyper::header::{HOST, HeaderMap, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};

/// Where the app listens; `None` — not reachable now (e.g. the container is gone).
pub type Target = Arc<dyn Fn() -> Option<(String, u16)> + Send + Sync>;

struct Preview {
    port: u16,
    target: Arc<Mutex<Target>>,
    /// The key the proxy files its logs under (it changes when the port is taken over).
    key: Arc<Mutex<String>>,
    /// When it was last asked for or served a request (ms since `START`).
    used: Arc<AtomicU64>,
    server: JoinHandle<()>,
}

static PREVIEWS: LazyLock<Mutex<HashMap<String, Preview>>> = LazyLock::new(Default::default);
static START: LazyLock<Instant> = LazyLock::new(Instant::now);

fn now_ms() -> u64 {
    START.elapsed().as_millis() as u64
}

// ---------- the helper script inside previewed pages, and what it reports ----------

/// The script the proxy adds to every HTML page (console, errors, address bar, element picker).
const INJECT_JS: &str = include_str!("../../../packages/shared/assets/preview-inject.js");
const INJECT_TAG: &str = r#"<script src="/__hubness/preview.js"></script>"#;
/// The hub's headless browser (preview_* agent tools) marks itself in the user agent.
pub const AGENT_UA_MARK: &str = "hubness-agent";
const MAX_LOGS: usize = 500;
/// How much of a page is looked at for the place of the helper before it is passed on as is.
const HEAD_LIMIT: usize = 64 * 1024;

static LOGS: LazyLock<Mutex<HashMap<String, Vec<PreviewLog>>>> = LazyLock::new(Default::default);
/// One counter for the whole hub: a cleared log or a restarted preview never goes back to 1
/// (clients keep a cursor).
static LOG_SEQ: AtomicU64 = AtomicU64::new(0);

fn inject_on() -> bool {
    std::env::var("HUBNESS_PREVIEW_INJECT").map(|v| v != "0").unwrap_or(true)
}

/// Cut to `max` UTF-16 units (JS `slice`).
fn cut(s: &str, max: usize) -> String {
    let mut units = 0;
    s.chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= max
        })
        .collect()
}

/// A reported field as text: a string as it is, nothing as "", anything else as JSON.
fn js_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Keep one reported entry (`raw` as the page sent it).
pub fn add_preview_log(key: &str, raw: &Value, source: &str) {
    let level = raw.get("level").and_then(Value::as_str).filter(|l| ["error", "warn", "log", "network"].contains(l)).unwrap_or("log");
    let ts = match raw.get("ts") {
        Some(Value::String(t)) => cut(t, 40),
        _ => crate::db::now(),
    };
    let mut logs = lock(&LOGS);
    let list = logs.entry(key.to_string()).or_default();
    list.push(PreviewLog {
        // under the lock: the entries of a list stay in seq order
        seq: LOG_SEQ.fetch_add(1, Ordering::SeqCst) + 1,
        ts,
        level: level.into(),
        text: cut(&js_string(raw.get("text").unwrap_or(&Value::Null)), 4000),
        url: cut(&js_string(raw.get("url").unwrap_or(&Value::Null)), 2000),
        source: source.into(),
    });
    if list.len() > MAX_LOGS {
        let extra = list.len() - MAX_LOGS;
        list.drain(..extra);
    }
}

/// Log entries of a preview after `since` (a seq).
pub fn preview_logs(key: &str, since: u64) -> Vec<PreviewLog> {
    lock(&LOGS).get(key).map(|l| l.iter().filter(|l| l.seq > since).cloned().collect()).unwrap_or_default()
}

pub fn clear_preview_logs(key: &str) {
    lock(&LOGS).remove(key);
}

/// Is the port one of `HUBNESS_PREVIEW_PORTS` (a proxy, never an app)?
pub fn in_preview_range(port: u16) -> bool {
    let (from, to) = range();
    (from..=to).contains(&port)
}

pub fn preview_key_by_port(port: u16) -> Option<String> {
    lock(&PREVIEWS).iter().find(|(_, p)| p.port == port).map(|(k, _)| k.clone())
}

fn head_tag() -> &'static regex::bytes::Regex {
    static HEAD: LazyLock<regex::bytes::Regex> = LazyLock::new(|| regex::bytes::Regex::new(r"(?i-u)<head(\s[^>]*)?>").unwrap());
    &HEAD
}

/// Where the helper tag goes in the start of a document: right after `<head>`, else after
/// the doctype or `<html>` it begins with. `None` — this is not a document (a fragment for
/// jQuery / htmx, JSON with a wrong type): it is left alone.
pub fn inject_point(html: &[u8]) -> Option<usize> {
    static START: LazyLock<regex::bytes::Regex> =
        LazyLock::new(|| regex::bytes::Regex::new(r"(?i-u)^\s*(<!doctype[^>]*>|<html(\s[^>]*)?>)").unwrap());
    head_tag().find(html).or_else(|| START.find(html)).map(|m| m.end())
}

/// A document with the helper tag (unchanged when it has one, or is not a document). Works
/// on the raw bytes: a page in another encoding than UTF-8 keeps every byte.
pub fn inject_html(html: &[u8]) -> Vec<u8> {
    if html.windows(b"/__hubness/preview.js".len()).any(|w| w == b"/__hubness/preview.js") {
        return html.to_vec();
    }
    match inject_point(html) {
        Some(at) => [&html[..at], INJECT_TAG.as_bytes(), &html[at..]].concat(),
        None => html.to_vec(),
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Is it the browser loading a page (not a script asking for a piece of HTML)?
fn page_request(method: &hyper::Method, headers: &HeaderMap) -> bool {
    if !inject_on() || method != hyper::Method::GET {
        return false;
    }
    match header(headers, "sec-fetch-dest") {
        Some(dest) => matches!(dest, "document" | "iframe" | "frame"),
        None => header(headers, "accept").is_some_and(|a| a.contains("text/html")),
    }
}

/// Only the previewed page itself may report: another site in the user's browser can POST
/// plain text here blindly, and what lands in the log is read by the agent.
fn same_origin(headers: &HeaderMap) -> bool {
    if header(headers, "sec-fetch-site").is_some_and(|s| s != "same-origin" && s != "none") {
        return false;
    }
    if !headers.contains_key("origin") {
        return true;
    }
    // scheme://host[:port] — the host part must be the Host the request came to
    let Some(origin) = header(headers, "origin") else { return false };
    let host = origin.split_once("://").map(|(_, h)| h).filter(|h| !h.is_empty() && !h.contains('/'));
    host.is_some() && host.map(str::to_ascii_lowercase) == header(headers, "host").map(str::to_ascii_lowercase)
}

/// The preview port range from `HUBNESS_PREVIEW_PORTS` (`from-to`).
fn range() -> (u16, u16) {
    static RANGE: LazyLock<(u16, u16)> = LazyLock::new(|| {
        let raw = std::env::var("HUBNESS_PREVIEW_PORTS").unwrap_or_else(|_| "8790-8809".into());
        let mut it = raw.split('-').map(|p| p.trim().parse::<u16>().unwrap_or(0));
        let from = it.next().unwrap_or(0);
        (from, it.next().unwrap_or(0))
    });
    *RANGE
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A preview of a port somebody named (not of a run command): `<project>:port:<n>`.
fn ad_hoc(key: &str) -> bool {
    key.contains(":port:")
}

/// Preview port for a key (project:app port); starts the proxy on first request. When every
/// port is taken, the ad-hoc preview nobody has used for the longest time gives its up (an
/// agent trying addresses would otherwise use the range up for good).
/// Must be called inside the tokio runtime.
pub fn ensure_preview(host: &str, key: &str, target: Target) -> Option<u16> {
    ensure_in(host, range(), key, target)
}

fn ensure_in(host: &str, (from, to): (u16, u16), key: &str, target: Target) -> Option<u16> {
    let mut previews = lock(&PREVIEWS);
    if let Some(cur) = previews.get(key) {
        *lock(&cur.target) = target;
        cur.used.store(now_ms(), Ordering::SeqCst);
        // asked for again: one more try if Tailscale Serve refused the port before
        crate::tailscale::expose(cur.port);
        return Some(cur.port);
    }
    let used: Vec<u16> = previews.values().map(|p| p.port).collect();
    for port in from..=to {
        if port == 0 || used.contains(&port) {
            continue;
        }
        let Ok(std_listener) = std::net::TcpListener::bind((host, port)) else { continue };
        if std_listener.set_nonblocking(true).is_err() {
            continue;
        }
        let Ok(listener) = TcpListener::from_std(std_listener) else { continue };
        let target = Arc::new(Mutex::new(target));
        let key_cell = Arc::new(Mutex::new(key.to_string()));
        let used = Arc::new(AtomicU64::new(now_ms()));
        let server = tokio::spawn(accept_loop(listener, port, key_cell.clone(), target.clone(), used.clone()));
        previews.insert(key.to_string(), Preview { port, target, key: key_cell, used, server });
        crate::tailscale::expose(port);
        return Some(port);
    }
    // nothing free: the idlest ad-hoc preview of this range hands its port (and listener) over
    let idle = previews
        .iter()
        .filter(|(k, p)| ad_hoc(k) && (from..=to).contains(&p.port))
        .min_by_key(|(_, p)| p.used.load(Ordering::SeqCst))
        .map(|(k, _)| k.clone())?;
    let p = previews.remove(&idle)?;
    lock(&LOGS).remove(&idle);
    *lock(&p.target) = target;
    *lock(&p.key) = key.to_string();
    p.used.store(now_ms(), Ordering::SeqCst);
    let port = p.port;
    previews.insert(key.to_string(), p);
    Some(port)
}

pub fn preview_port(key: &str) -> Option<u16> {
    lock(&PREVIEWS).get(key).map(|p| p.port)
}

/// Stop a preview: the listener and every open connection.
pub fn close_preview(key: &str) {
    clear_preview_logs(key);
    if let Some(p) = lock(&PREVIEWS).remove(key) {
        // the accept loop owns the connection tasks: aborting it drops (aborts) them all
        p.server.abort();
        crate::tailscale::unexpose(p.port);
    }
}

pub fn close_previews_with_prefix(prefix: &str) {
    let keys: Vec<String> = lock(&PREVIEWS).keys().filter(|k| k.starts_with(prefix)).cloned().collect();
    for k in keys {
        close_preview(&k);
    }
}

async fn accept_loop(listener: TcpListener, port: u16, key: Arc<Mutex<String>>, target: Arc<Mutex<Target>>, used: Arc<AtomicU64>) {
    let mut conns = JoinSet::new();
    loop {
        while conns.try_join_next().is_some() {}
        let Ok((socket, _)) = listener.accept().await else { continue };
        let (target, key, used) = (target.clone(), key.clone(), used.clone());
        conns.spawn(async move {
            let service = hyper::service::service_fn(move |req| {
                used.store(now_ms(), Ordering::SeqCst);
                let t = lock(&target).clone();
                let k = lock(&key).clone();
                proxy(req, port, k, t)
            });
            let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket), service).with_upgrades().await;
        });
    }
}

fn text(status: StatusCode, body: String) -> Response<Body> {
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    res.headers_mut().insert("content-type", HeaderValue::from_static("text/plain; charset=utf-8"));
    res
}

fn empty(status: StatusCode) -> Response<Body> {
    let mut res = Response::new(Body::empty());
    *res.status_mut() = status;
    res
}

/// The proxy's own paths: the helper script and its log endpoint.
async fn own_route(key: &str, req: Request<Incoming>) -> Result<Response<Body>, Box<Request<Incoming>>> {
    let path = req.uri().path();
    if path == "/__hubness/preview.js" && req.method() == hyper::Method::GET {
        let mut res = Response::new(Body::from(INJECT_JS));
        res.headers_mut().insert("content-type", HeaderValue::from_static("text/javascript; charset=utf-8"));
        res.headers_mut().insert("cache-control", HeaderValue::from_static("no-store"));
        return Ok(res);
    }
    if path == "/__hubness/log" && req.method() == hyper::Method::POST {
        if !same_origin(req.headers()) {
            return Ok(empty(StatusCode::FORBIDDEN));
        }
        let ua = req.headers().get(hyper::header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
        let source = if ua.contains(AGENT_UA_MARK) { "agent" } else { "user" };
        let body = http_body_util::Limited::new(req.into_body(), 256_000).collect().await.map(|b| b.to_bytes()).unwrap_or_default();
        if let Ok(Value::Array(list)) = serde_json::from_slice::<Value>(&body) {
            for e in list.iter().take(50).filter(|e| e.is_object()) {
                add_preview_log(key, e, source);
            }
        }
        return Ok(empty(StatusCode::NO_CONTENT));
    }
    Err(Box::new(req))
}

/// Pass an HTML answer on with the helper tag in it. Only the start is held back (until
/// `<head>` shows up, or `HEAD_LIMIT`): a streamed page keeps streaming, a page that never
/// ends is still shown, and bytes are never re-encoded.
async fn relay_html(mut parts: hyper::http::response::Parts, mut body: Incoming) -> Response<Body> {
    let mut held: Vec<u8> = vec![];
    let mut ended = false;
    let mut broken = false;
    while !(head_tag().is_match(&held) || held.len() >= HEAD_LIMIT) {
        match body.frame().await {
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    held.extend_from_slice(&data);
                }
            }
            Some(Err(_)) => {
                broken = true;
                break;
            }
            None => {
                ended = true;
                break;
            }
        }
    }
    let out = inject_html(&held);
    let added = out.len() - held.len();
    // a known length grows by the tag; a chunked answer stays chunked
    if added > 0
        && let Some(len) = header(&parts.headers, "content-length").and_then(|v| v.parse::<usize>().ok())
    {
        parts.headers.insert(hyper::header::CONTENT_LENGTH, HeaderValue::from(len + added));
    }
    if ended {
        return Response::from_parts(parts, Body::from(out));
    }
    let first = futures_util::stream::once(async move { Ok::<_, axum::Error>(Bytes::from(out)) });
    let body = if broken {
        // the app died mid-answer: pass the cut on (the connection is dropped), not a hang
        Body::from_stream(first.chain(futures_util::stream::once(async { Err(axum::Error::new("the response broke off")) })))
    } else {
        Body::from_stream(first.chain(body.into_data_stream().map(|r| r.map_err(axum::Error::new))))
    };
    Response::from_parts(parts, body)
}

async fn proxy(req: Request<Incoming>, own_port: u16, key: String, target: Target) -> Result<Response<Body>, std::convert::Infallible> {
    // preview ports are as reachable as the hub: the same sign-in (the browser's session
    // cookie), or the agents' browser with its pass for this port
    if !crate::auth::authorized(req.headers(), own_port) {
        return Ok(text(StatusCode::UNAUTHORIZED, "Sign in to hubness first, then reload this preview.".into()));
    }
    let mut req = match own_route(&key, req).await {
        Ok(res) => return Ok(res),
        Err(req) => *req,
    };
    crate::auth::strip_credentials(req.headers_mut());
    // a page (not an asset) comes uncompressed, so the helper can be added to it
    let page = page_request(req.method(), req.headers());
    if page {
        req.headers_mut().insert(hyper::header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    }
    // the target may ask docker for the container address: off the async threads
    let Ok(Some((host, port))) = tokio::task::spawn_blocking(move || target()).await else {
        return Ok(text(StatusCode::SERVICE_UNAVAILABLE, "The app is not running. Start it in hubness (Run).".into()));
    };
    let fail = |e: String| {
        text(StatusCode::BAD_GATEWAY, format!("The app doesn’t respond on port {port} yet ({e}). Wait for it to start and reload."))
    };
    let original_host = req.headers().get(HOST).cloned().unwrap_or_else(|| HeaderValue::from_static(""));
    let upgrade = req.headers().contains_key(hyper::header::UPGRADE);
    {
        let h = req.headers_mut();
        if let Ok(v) = HeaderValue::from_str(&format!("localhost:{port}")) {
            h.insert(HOST, v);
        }
        if !upgrade {
            h.insert("x-forwarded-host", original_host);
            h.insert("x-forwarded-proto", HeaderValue::from_static("http"));
        }
    }
    let stream = match TcpStream::connect((host.as_str(), port)).await {
        Ok(s) => s,
        Err(e) => return Ok(fail(e.to_string())),
    };
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(TokioIo::new(stream)).await {
        Ok(v) => v,
        Err(e) => return Ok(fail(e.to_string())),
    };
    tokio::spawn(conn.with_upgrades());

    // WebSocket (HMR etc.): forward the handshake, then pass bytes through both ways
    let client_upgrade = upgrade.then(|| hyper::upgrade::on(&mut req));
    let mut res = match sender.send_request(req).await {
        Ok(r) => r,
        Err(e) => return Ok(fail(e.to_string())),
    };
    if res.status() == StatusCode::SWITCHING_PROTOCOLS
        && let Some(client_upgrade) = client_upgrade
    {
        let upstream = hyper::upgrade::on(&mut res);
        tokio::spawn(async move {
            if let (Ok(client), Ok(upstream)) = (client_upgrade.await, upstream.await) {
                let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(client), &mut TokioIo::new(upstream)).await;
            }
        });
        let (parts, _) = res.into_parts();
        return Ok(Response::from_parts(parts, Body::empty()));
    }
    let (parts, body) = res.into_parts();
    let html = page
        && parts.status != StatusCode::NO_CONTENT
        && parts.status != StatusCode::NOT_MODIFIED
        && header(&parts.headers, "content-type").is_some_and(|c| c.contains("text/html"))
        && !parts.headers.contains_key(hyper::header::CONTENT_ENCODING);
    if html {
        return Ok(relay_html(parts, body).await);
    }
    Ok(Response::from_parts(parts, Body::new(body.map_err(axum::Error::new))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn tagged(parts: &[&[u8]]) -> Vec<u8> {
        parts.join(INJECT_TAG.as_bytes())
    }

    #[test]
    fn the_helper_goes_after_head_else_after_the_doctype_or_html_and_fragments_are_left_alone() {
        assert_eq!(
            inject_html(b"<html><head><title>x</title></head></html>"),
            tagged(&[b"<html><head>", b"<title>x</title></head></html>"])
        );
        assert_eq!(inject_html(b"<HTML><HEAD lang=\"en\">x"), tagged(&[b"<HTML><HEAD lang=\"en\">", b"x"]), "any case, with attributes");
        assert_eq!(inject_html(b"<!DOCTYPE html>\n<body>hi</body>"), tagged(&[b"<!DOCTYPE html>", b"\n<body>hi</body>"]));
        assert_eq!(
            inject_html(b"\n<html lang=\"en\"><body>hi</body></html>"),
            tagged(&[b"\n<html lang=\"en\">", b"<body>hi</body></html>"])
        );
        // every byte of another encoding survives
        assert_eq!(inject_html(b"<head>\xe9\xff"), tagged(&[b"<head>", b"\xe9\xff"]));
        // a piece of HTML for jQuery .load / htmx / Turbo is not a page
        for fragment in
            ["<p>fragment</p>", "<li>item</li><li>two</li>", "", "<header>not a head</header>", "{\"html\":false}", "text <!doctype html>"]
        {
            assert_eq!(inject_html(fragment.as_bytes()), fragment.as_bytes(), "{fragment}");
        }
        assert_eq!(inject_point(b"<html><head>"), Some(12));
        assert_eq!(inject_point(b"<p>"), None);
        let once = inject_html(b"<head></head>");
        assert_eq!(inject_html(&once), once, "never twice");
    }

    #[test]
    fn reported_logs_are_a_bounded_ring_with_one_counter_for_the_hub() {
        let key = "unit:logs";
        add_preview_log(
            key,
            &serde_json::json!({ "level": "error", "text": "a", "url": "http://x/", "ts": "2026-01-01T00:00:00.000Z" }),
            "user",
        );
        add_preview_log(key, &serde_json::json!({ "level": "nope", "text": 5 }), "agent");
        add_preview_log(key, &serde_json::json!({ "text": { "a": 1 } }), "user");
        let list = preview_logs(key, 0);
        let first = list[0].seq;
        assert_eq!(
            (list[0].ts.as_str(), list[0].level.as_str(), list[0].text.as_str(), list[0].url.as_str(), list[0].source.as_str()),
            ("2026-01-01T00:00:00.000Z", "error", "a", "http://x/", "user")
        );
        assert_eq!((list[1].level.as_str(), list[1].text.as_str(), list[1].source.as_str()), ("log", "5", "agent"));
        assert_eq!((list[2].text.as_str(), list[2].url.as_str()), ("{\"a\":1}", ""));
        assert!(list[1].ts.starts_with("20"), "a missing time is now");
        assert!(list.windows(2).all(|w| w[0].seq < w[1].seq));
        assert_eq!(preview_logs(key, list[1].seq).len(), 1);
        assert!(preview_logs("unit:other", 0).is_empty(), "previews do not share logs");

        let long = serde_json::json!({ "text": "x".repeat(5000), "url": "u".repeat(3000), "ts": "t".repeat(100) });
        for _ in 0..600 {
            add_preview_log(key, &long, "user");
        }
        let full = preview_logs(key, 0);
        assert_eq!(full.len(), 500, "old entries fall out");
        assert!(full.last().unwrap().seq >= first + 602, "seq keeps counting");
        assert_eq!((full[0].text.len(), full[0].url.len(), full[0].ts.len()), (4000, 2000, 40));

        // a client keeps a cursor: nothing that comes after a Clear (or in another preview's
        // place) may hide behind it
        let cursor = full.last().unwrap().seq;
        clear_preview_logs(key);
        assert!(preview_logs(key, 0).is_empty());
        add_preview_log(key, &serde_json::json!({ "text": "after clear" }), "user");
        assert_eq!(preview_logs(key, cursor).iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["after clear"]);
        add_preview_log("unit:other", &serde_json::json!({ "text": "elsewhere" }), "user");
        assert!(preview_logs("unit:other", 0)[0].seq > preview_logs(key, 0)[0].seq, "one counter for the hub");
        clear_preview_logs(key);
        clear_preview_logs("unit:other");
    }

    fn headers(list: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in list {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn pages_are_navigations_and_only_the_page_itself_may_report() {
        let get = hyper::Method::GET;
        assert!(page_request(&get, &headers(&[("accept", "text/html,application/xhtml+xml")])));
        assert!(!page_request(&hyper::Method::POST, &headers(&[("accept", "text/html")])), "only GET");
        assert!(!page_request(&get, &headers(&[("accept", "application/json")])));
        assert!(
            !page_request(&get, &headers(&[("accept", "text/html"), ("sec-fetch-dest", "empty")])),
            "fetch() of a page is not a navigation"
        );
        assert!(page_request(&get, &headers(&[("accept", "*/*"), ("sec-fetch-dest", "iframe")])), "the Preview panel's frame is");
        assert!(page_request(&get, &headers(&[("sec-fetch-dest", "document")])));

        let host = ("host", "127.0.0.1:8790");
        assert!(same_origin(&headers(&[host])), "no browser headers: a plain client");
        assert!(same_origin(&headers(&[host, ("sec-fetch-site", "same-origin"), ("origin", "http://127.0.0.1:8790")])));
        assert!(same_origin(&headers(&[host, ("sec-fetch-site", "none")])));
        assert!(!same_origin(&headers(&[host, ("origin", "https://evil.example")])));
        assert!(!same_origin(&headers(&[host, ("sec-fetch-site", "cross-site")])));
        assert!(
            !same_origin(&headers(&[host, ("sec-fetch-site", "same-site"), ("origin", "http://127.0.0.1:8790")])),
            "same-site is not same-origin"
        );
        assert!(!same_origin(&headers(&[host, ("origin", "http://127.0.0.1:8787")])), "another port of this host is another origin");
        assert!(!same_origin(&headers(&[host, ("origin", "null")])));
    }

    /// An app on a free port; `handler` gets the request line + headers and writes the raw answer.
    async fn app<F, Fut>(handler: F) -> u16
    where
        F: Fn(String, TcpStream) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handler = Arc::new(handler);
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let handler = handler.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        if s.read(&mut byte).await.unwrap_or(0) == 0 {
                            return;
                        }
                        head.push(byte[0]);
                    }
                    handler(String::from_utf8_lossy(&head).into_owned(), s).await;
                });
            }
        });
        port
    }

    /// One request to a preview port over a fresh connection: the raw answer.
    async fn ask(port: u16, request: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(request.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out).await;
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn proxies_from_the_root_with_host_rewritten_and_nothing_of_the_hubs_passed_on() {
        // the app answers with what it was asked
        let app_port = app(|head, mut s| async move {
            let body = head.replace("\r\n", "|");
            let _ = s
                .write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes())
                .await;
        })
        .await;
        let target: Target = Arc::new(move || Some(("127.0.0.1".into(), app_port)));
        let range = (18950, 18959);
        let port = ensure_in("127.0.0.1", range, "t:1", target.clone()).expect("a free preview port");
        assert_eq!(ensure_in("127.0.0.1", range, "t:1", target), Some(port), "same key — same port");
        assert_eq!(preview_key_by_port(port).as_deref(), Some("t:1"));

        let pass = crate::auth::preview_pass(port);
        let cookies = format!("Cookie: a=1; hubness_session=SECRET-SESSION; hubness_preview={pass}; b=2\r\n");
        let out = ask(port, &format!("GET /assets/app.js?x=1 HTTP/1.1\r\nHost: example\r\n{cookies}Connection: close\r\n\r\n")).await;
        assert!(out.contains("GET /assets/app.js?x=1 HTTP/1.1|"), "{out}");
        assert!(out.to_lowercase().contains(&format!("|host: localhost:{app_port}|")), "{out}");
        assert!(out.to_lowercase().contains("|cookie: a=1; b=2|"), "the app's own cookies go through: {out}");
        assert!(!out.contains("SECRET-SESSION") && !out.contains(&pass), "the hub's session and the preview pass do not: {out}");

        // the same on the way to a WebSocket (the handshake is forwarded as a request)
        let ws =
            ask(port, &format!("GET /hmr HTTP/1.1\r\nHost: example\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n{cookies}\r\n")).await;
        assert!(ws.to_lowercase().contains("|upgrade: websocket|"), "{ws}");
        assert!(ws.to_lowercase().contains("|cookie: a=1; b=2|"), "{ws}");
        assert!(!ws.contains("SECRET-SESSION") && !ws.contains(&pass), "{ws}");

        close_preview("t:1");
        assert_eq!(preview_port("t:1"), None);
    }

    #[tokio::test]
    async fn a_page_that_streams_is_shown_as_it_comes_and_lengths_stay_true() {
        let (go_tx, go_rx) = tokio::sync::watch::channel(false);
        let app_port = app(move |head, mut s| {
            let mut go = go_rx.clone();
            async move {
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let chunk = |t: &str| format!("{:x}\r\n{t}\r\n", t.len());
                match path.as_str() {
                    "/stream" => {
                        let first = chunk("<!doctype html><html><head><title>s</title></head><body>first");
                        let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n{first}").as_bytes()).await;
                        let _ = go.wait_for(|g| *g).await;
                        let _ = s.write_all(format!("{}0\r\n\r\n", chunk(" last</body></html>")).as_bytes()).await;
                    }
                    "/sized" => {
                        let body = "<html><head></head><body>\u{436}</body></html>";
                        let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
                    }
                    "/fragment" => {
                        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 13\r\nConnection: close\r\n\r\n<li>item</li>").await;
                    }
                    "/empty304" => {
                        let _ = s.write_all(b"HTTP/1.1 304 Not Modified\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n").await;
                    }
                    "/dies" => {
                        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 1000\r\n\r\n<html><head></head>partial").await;
                    }
                    _ => {
                        let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Type: text/html\r\nContent-Length: 20\r\nConnection: close\r\n\r\n<head></head>missing").await;
                    }
                }
            }
        })
        .await;
        let target: Target = Arc::new(move || Some(("127.0.0.1".into(), app_port)));
        let port = ensure_in("127.0.0.1", (18960, 18969), "t:stream", target).expect("a free preview port");
        let get =
            |path: &str, extra: &str| format!("GET {path} HTTP/1.1\r\nHost: x\r\nAccept: text/html\r\n{extra}Connection: close\r\n\r\n");

        // the start of the page arrives while the app is still writing the rest
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(get("/stream", "").as_bytes()).await.unwrap();
        let mut seen = String::new();
        let mut buf = [0u8; 4096];
        let early = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !seen.contains("first") {
                let n = s.read(&mut buf).await.unwrap();
                assert!(n > 0, "closed early: {seen}");
                seen.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        })
        .await;
        assert!(early.is_ok(), "the first part came before the app finished: {seen}");
        assert!(seen.contains(&format!("<head>{INJECT_TAG}<title>s</title>")), "{seen}");
        assert!(!seen.contains("last"));
        assert!(!seen.to_lowercase().contains("content-length"), "no length was given: none is made up — {seen}");
        go_tx.send(true).unwrap();
        let mut rest = String::new();
        let _ = s.read_to_string(&mut rest).await;
        assert!(rest.contains(" last</body></html>"), "{rest}");

        let sized = ask(port, &get("/sized", "")).await;
        let body = "<html><head></head><body>\u{436}</body></html>";
        assert!(
            sized.to_lowercase().contains(&format!("content-length: {}\r\n", body.len() + INJECT_TAG.len())),
            "a given length grows by the tag: {sized}"
        );
        assert!(sized.ends_with(&body.replace("<head>", &format!("<head>{INJECT_TAG}"))), "{sized}");

        let fragment = ask(port, &get("/fragment", "")).await;
        assert!(
            fragment.ends_with("\r\n\r\n<li>item</li>") && fragment.to_lowercase().contains("content-length: 13\r\n"),
            "a fragment is not a page: {fragment}"
        );
        let fetched = ask(port, &get("/sized", "Sec-Fetch-Dest: empty\r\n")).await;
        assert!(fetched.ends_with(body), "fetch() of a page is not a navigation: {fetched}");
        let not_modified = ask(port, &get("/empty304", "")).await;
        assert!(not_modified.starts_with("HTTP/1.1 304") && !not_modified.contains("script"), "{not_modified}");
        let missing = ask(port, &get("/nope", "")).await;
        assert!(
            missing.starts_with("HTTP/1.1 404") && missing.ends_with(&format!("<head>{INJECT_TAG}</head>missing")),
            "error pages report too: {missing}"
        );
        let post =
            ask(port, "POST /sized HTTP/1.1\r\nHost: x\r\nAccept: text/html\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
        assert!(post.ends_with(body), "only GET: {post}");

        // the app dies mid-answer: the answer ends, short of the promised length
        let cut = tokio::time::timeout(std::time::Duration::from_secs(5), ask(port, &get("/dies", ""))).await.expect("no hang");
        assert!(cut.contains("partial") && cut.len() < 1000, "{cut}");

        // the log endpoint: the page itself, not another site
        let log = |extra: &str| {
            let body = r#"[{"level":"error","text":"x"}]"#;
            format!(
                "POST /__hubness/log HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        };
        assert!(ask(port, &log("Origin: https://evil.example\r\n")).await.starts_with("HTTP/1.1 403"));
        assert!(ask(port, &log("Sec-Fetch-Site: cross-site\r\n")).await.starts_with("HTTP/1.1 403"));
        assert!(preview_logs("t:stream", 0).is_empty());
        assert!(
            ask(port, &log(&format!("Sec-Fetch-Site: same-origin\r\nOrigin: http://127.0.0.1:{port}\r\n")))
                .await
                .starts_with("HTTP/1.1 204")
        );
        assert_eq!(preview_logs("t:stream", 0).len(), 1);

        close_preview("t:stream");
        assert!(preview_logs("t:stream", 0).is_empty(), "logs go with the preview");
    }

    #[tokio::test]
    async fn when_the_ports_run_out_the_idlest_ad_hoc_preview_gives_its_up() {
        let target: Target = Arc::new(|| Some(("127.0.0.1".into(), 1)));
        let range = (18970, 18972);
        let run = ensure_in("127.0.0.1", range, "lru:run", target.clone()).unwrap();
        let first = ensure_in("127.0.0.1", range, "lru:port:1", target.clone()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let second = ensure_in("127.0.0.1", range, "lru:port:2", target.clone()).unwrap();
        add_preview_log("lru:port:1", &serde_json::json!({ "text": "old" }), "user");
        // asking for the first again makes the second the idle one
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        assert_eq!(ensure_in("127.0.0.1", range, "lru:port:1", target.clone()), Some(first));
        let third =
            ensure_in("127.0.0.1", range, "lru:port:3", target.clone()).expect("a preview is always found while ad-hoc ones can give way");
        assert_eq!(third, second, "the port of the one used longest ago");
        assert_eq!(preview_port("lru:port:2"), None);
        assert_eq!(preview_key_by_port(third).as_deref(), Some("lru:port:3"));
        assert_eq!((preview_port("lru:run"), preview_port("lru:port:1")), (Some(run), Some(first)), "a running command keeps its preview");
        // the proxy on that port now serves (and files its logs under) the new key
        let body = r#"[{"text":"new"}]"#;
        let out = ask(
            third,
            &format!("POST /__hubness/log HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()),
        )
        .await;
        assert!(out.starts_with("HTTP/1.1 204"), "{out}");
        assert_eq!(preview_logs("lru:port:3", 0).len(), 1);
        assert!(preview_logs("lru:port:2", 0).is_empty());

        // only runs left: nothing to take
        close_preview("lru:port:1");
        close_preview("lru:port:3");
        assert!(preview_logs("lru:port:1", 0).is_empty());
        // a closed listener is gone once its (aborted) task was dropped
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let a = ensure_in("127.0.0.1", range, "lru:run2", target.clone());
        let b = ensure_in("127.0.0.1", range, "lru:run3", target.clone());
        assert!(a.is_some() && b.is_some());
        assert_eq!(ensure_in("127.0.0.1", range, "lru:run4", target.clone()), None);
        assert_eq!(ensure_in("127.0.0.1", range, "lru:port:9", target), None, "an ad-hoc one does not push a run out");
        close_previews_with_prefix("lru:");
    }
}
