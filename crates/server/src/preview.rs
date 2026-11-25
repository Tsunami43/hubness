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

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::header::{HOST, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};

/// Where the app listens; `None` — not reachable now (e.g. the container is gone).
pub type Target = Arc<dyn Fn() -> Option<(String, u16)> + Send + Sync>;

struct Preview {
    port: u16,
    target: Arc<Mutex<Target>>,
    server: JoinHandle<()>,
}

static PREVIEWS: LazyLock<Mutex<HashMap<String, Preview>>> = LazyLock::new(Default::default);

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

/// Preview port for a key (project:app port); starts the proxy on first request.
/// Must be called inside the tokio runtime.
pub fn ensure_preview(host: &str, key: &str, target: Target) -> Option<u16> {
    let mut previews = lock(&PREVIEWS);
    if let Some(cur) = previews.get(key) {
        *lock(&cur.target) = target;
        return Some(cur.port);
    }
    let used: Vec<u16> = previews.values().map(|p| p.port).collect();
    let (from, to) = range();
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
        let server = tokio::spawn(accept_loop(listener, target.clone()));
        previews.insert(key.to_string(), Preview { port, target, server });
        return Some(port);
    }
    None
}

pub fn preview_port(key: &str) -> Option<u16> {
    lock(&PREVIEWS).get(key).map(|p| p.port)
}

/// Stop a preview: the listener and every open connection.
pub fn close_preview(key: &str) {
    if let Some(p) = lock(&PREVIEWS).remove(key) {
        // the accept loop owns the connection tasks: aborting it drops (aborts) them all
        p.server.abort();
    }
}

pub fn close_previews_with_prefix(prefix: &str) {
    let keys: Vec<String> = lock(&PREVIEWS).keys().filter(|k| k.starts_with(prefix)).cloned().collect();
    for k in keys {
        close_preview(&k);
    }
}

async fn accept_loop(listener: TcpListener, target: Arc<Mutex<Target>>) {
    let mut conns = JoinSet::new();
    loop {
        while conns.try_join_next().is_some() {}
        let Ok((socket, _)) = listener.accept().await else { continue };
        let target = target.clone();
        conns.spawn(async move {
            let service = hyper::service::service_fn(move |req| {
                let t = lock(&target).clone();
                proxy(req, t)
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

async fn proxy(mut req: Request<Incoming>, target: Target) -> Result<Response<Body>, std::convert::Infallible> {
    // preview ports are as reachable as the hub: the same sign-in (the browser's session cookie)
    if !crate::auth::authorized(req.headers()) {
        return Ok(text(StatusCode::UNAUTHORIZED, "Sign in to hubness first, then reload this preview.".into()));
    }
    crate::auth::strip_credentials(req.headers_mut());
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
    Ok(Response::from_parts(parts, Body::new(body.map_err(axum::Error::new))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn proxies_from_the_root_with_host_rewritten() {
        let app = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let app_port = app.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (s, _) = app.accept().await.unwrap();
                let svc = hyper::service::service_fn(|req: Request<Incoming>| async move {
                    let host = req.headers().get(HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
                    Ok::<_, std::convert::Infallible>(Response::new(Body::from(format!("{} {host}", req.uri()))))
                });
                tokio::spawn(hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(s), svc));
            }
        });
        let target: Target = Arc::new(move || Some(("127.0.0.1".into(), app_port)));
        let port = ensure_preview("127.0.0.1", "t:1", target.clone()).expect("a free preview port");
        assert_eq!(ensure_preview("127.0.0.1", "t:1", target), Some(port), "same key — same port");
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(b"GET /assets/app.js?x=1 HTTP/1.1\r\nHost: example\r\nConnection: close\r\n\r\n").await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        assert!(out.ends_with(&format!("/assets/app.js?x=1 localhost:{app_port}")), "{out}");
        close_preview("t:1");
        assert_eq!(preview_port("t:1"), None);
    }
}
