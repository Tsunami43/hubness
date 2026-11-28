//! Previews behind Tailscale Serve — a port of `tailscale.ts`.
//!
//! A hub opened as `https://<machine>.<tailnet>.ts.net` (`tailscale serve` → the hub's port)
//! is reached only through the ports Serve passes, and a preview is a port of its own: the
//! hub adds a Serve route for a preview port when the preview starts and removes it when the
//! preview stops. (Subdomains per preview are not an option there: MagicDNS and the
//! certificate cover the machine's own name only.)
//!
//! Needs the right to change Serve: `sudo tailscale set --operator=$USER`, once. Without it
//! the route is not added and the Preview tab says what to run. `HUBNESS_TAILSCALE` names the
//! `tailscale` binary (tests), `0` turns this off.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServeFront {
    /// Serve terminates TLS for the hub (`--https`), else plain HTTP (`--http`).
    pub https: bool,
}

/// Is the hub's port served by Tailscale Serve? Reads `tailscale serve status --json`.
pub fn serve_front(status: &Value, hub_port: u16) -> Option<ServeFront> {
    let web = status.get("Web")?.as_object()?;
    for (host_port, site) in web {
        let proxied = site.get("Handlers").and_then(Value::as_object).is_some_and(|handlers| {
            handlers.values().any(|h| h.get("Proxy").and_then(Value::as_str).is_some_and(|p| proxies_to(p, hub_port)))
        });
        if proxied {
            let port = host_port.rsplit(':').next().unwrap_or("");
            let https = status.get("TCP").and_then(|t| t.get(port)).and_then(|t| t.get("HTTPS")).and_then(Value::as_bool) == Some(true);
            return Some(ServeFront { https });
        }
    }
    None
}

/// `http://127.0.0.1:8787` (also `localhost`, `[::1]`, https, a trailing slash) → this port?
fn proxies_to(proxy: &str, hub_port: u16) -> bool {
    let Some(rest) = proxy.strip_prefix("http://").or_else(|| proxy.strip_prefix("https://")) else { return false };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let Some((host, port)) = rest.rsplit_once(':') else { return false };
    ["127.0.0.1", "localhost", "[::1]"].contains(&host) && port.parse() == Ok(hub_port)
}

enum Job {
    Expose(u16),
    Unexpose(u16),
    #[cfg_attr(not(test), allow(dead_code))]
    Flush(mpsc::Sender<()>),
}

struct Hub {
    tool: String,
    port: u16,
    host: String,
    jobs: Mutex<mpsc::Sender<Job>>,
}

static HUB: OnceLock<Hub> = OnceLock::new();
/// Preview ports that have a Serve route of ours.
static EXPOSED: LazyLock<Mutex<HashMap<u16, ServeFront>>> = LazyLock::new(Default::default);

const FRONT_TTL: Duration = Duration::from_secs(30);
const TIMEOUT: Duration = Duration::from_secs(10);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Called once by the server (tests of other modules never call it: then nothing here runs).
pub fn init(port: u16, host: &str) {
    let tool = std::env::var("HUBNESS_TAILSCALE").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| "tailscale".into());
    init_with(&tool, port, host);
}

fn init_with(tool: &str, port: u16, host: &str) {
    if tool == "0" || HUB.get().is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    if HUB.set(Hub { tool: tool.into(), port, host: host.into(), jobs: Mutex::new(tx) }).is_ok() {
        // one tailscale command at a time, in the order asked for
        std::thread::spawn(move || worker(rx));
    }
}

/// Run a tailscale command: did it succeed, and what it printed.
fn run(tool: &str, args: &[&str]) -> (bool, String) {
    let Ok(mut child) = Command::new(tool).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() else {
        return (false, String::new());
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return (false, "timed out".into());
            }
        }
    }
    match child.wait_with_output() {
        Ok(out) => (
            out.status.success(),
            format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string(),
        ),
        Err(_) => (false, String::new()),
    }
}

fn flag(front: ServeFront, port: u16) -> String {
    format!("--{}={port}", if front.https { "https" } else { "http" })
}

fn worker(rx: mpsc::Receiver<Job>) {
    let Some(hub) = HUB.get() else { return };
    let mut front: Option<(Instant, Option<ServeFront>)> = None;
    let mut warned = false;
    // where Serve finds the preview proxy: the hub's own address (loopback when it listens everywhere)
    let upstream = if ["0.0.0.0", "::"].contains(&hub.host.as_str()) { "127.0.0.1" } else { hub.host.as_str() };
    for job in rx {
        match job {
            Job::Expose(port) => {
                if lock(&EXPOSED).contains_key(&port) {
                    continue;
                }
                let current = match front {
                    Some((at, value)) if at.elapsed() < FRONT_TTL => value,
                    _ => {
                        let (ok, out) = run(&hub.tool, &["serve", "status", "--json"]);
                        let value = ok
                            .then(|| serde_json::from_str::<Value>(&out).ok())
                            .flatten()
                            .and_then(|status| serve_front(&status, hub.port));
                        front = Some((Instant::now(), value));
                        value
                    }
                };
                let Some(f) = current else { continue };
                let (ok, out) = run(&hub.tool, &["serve", "--bg", &flag(f, port), &format!("http://{upstream}:{port}")]);
                if ok {
                    lock(&EXPOSED).insert(port, f);
                } else if !warned {
                    warned = true;
                    eprintln!(
                        "hubness: could not open preview port {port} in Tailscale Serve ({}); run `sudo tailscale set --operator=$USER` once",
                        out.lines().next().unwrap_or("")
                    );
                }
            }
            Job::Unexpose(port) => {
                let f = lock(&EXPOSED).remove(&port);
                if let Some(f) = f {
                    run(&hub.tool, &["serve", &flag(f, port), "off"]);
                }
            }
            Job::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

fn send(job: Job) {
    if let Some(hub) = HUB.get() {
        let _ = lock(&hub.jobs).send(job);
    }
}

/// Let the preview port through Serve (nothing to do when the hub is not behind Serve).
pub fn expose(port: u16) {
    send(Job::Expose(port));
}

/// Take the preview port's Serve route away again.
pub fn unexpose(port: u16) {
    send(Job::Unexpose(port));
}

/// On hub exit: no route of ours stays behind.
pub fn shutdown() {
    let Some(hub) = HUB.get() else { return };
    let routes: Vec<(u16, ServeFront)> = lock(&EXPOSED).drain().collect();
    for (port, f) in routes {
        run(&hub.tool, &["serve", &flag(f, port), "off"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status(proxy: &str, https: bool) -> Value {
        json!({
            "TCP": { "443": if https { json!({ "HTTPS": true }) } else { json!({ "HTTP": true }) } },
            "Web": { "box.tail1234.ts.net:443": { "Handlers": { "/": { "Proxy": proxy } } } }
        })
    }

    #[test]
    fn the_hub_is_behind_serve_only_when_a_route_proxies_to_its_port() {
        assert_eq!(serve_front(&status("http://127.0.0.1:8787", true), 8787), Some(ServeFront { https: true }));
        assert_eq!(serve_front(&status("http://localhost:8787/", false), 8787), Some(ServeFront { https: false }));
        assert_eq!(serve_front(&status("http://127.0.0.1:3000", true), 8787), None, "another app");
        assert_eq!(serve_front(&status("http://10.0.0.5:8787", true), 8787), None, "another machine");
        assert_eq!(serve_front(&status("/srv/files", true), 8787), None);
        assert_eq!(serve_front(&json!({}), 8787), None);
        assert_eq!(serve_front(&json!(null), 8787), None);
    }

    /// The one test that starts the worker (`HUB` is set once per process): other tests'
    /// previews may then send their ports too, so only this test's ports are looked at.
    #[test]
    fn preview_ports_get_a_serve_route_and_lose_it() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let (log, tool) = (tmp.path().join("calls.log"), tmp.path().join("tailscale"));
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nif [ \"$1 $2\" = 'serve status' ]; then echo '{}'; fi\ncase \"$*\" in *=61002*) echo 'Access denied: serve config denied' >&2; exit 1;; esac\n",
            log.display(),
            status("http://127.0.0.1:61000", true)
        );
        std::fs::write(&tool, script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        init_with(tool.to_str().unwrap(), 61000, "127.0.0.1");

        let flush = || {
            let (tx, rx) = mpsc::channel();
            send(Job::Flush(tx));
            rx.recv_timeout(Duration::from_secs(20)).unwrap();
        };
        let calls = |port: &str| -> Vec<String> {
            std::fs::read_to_string(&log).unwrap_or_default().lines().filter(|l| l.contains(port)).map(String::from).collect()
        };
        expose(61001);
        expose(61001);
        flush();
        assert_eq!(calls("61001"), ["serve --bg --https=61001 http://127.0.0.1:61001"], "once");
        unexpose(61001);
        unexpose(61001);
        flush();
        assert_eq!(calls("61001").last().unwrap(), "serve --https=61001 off");
        assert_eq!(calls("61001").len(), 2);

        // refused (no operator rights): not ours, tried again when asked again, never removed
        expose(61002);
        expose(61002);
        unexpose(61002);
        flush();
        assert_eq!(calls("61002"), ["serve --bg --https=61002 http://127.0.0.1:61002"; 2]);

        expose(61003);
        flush();
        shutdown();
        assert_eq!(calls("61003").last().unwrap(), "serve --https=61003 off");
    }
}
