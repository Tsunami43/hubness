//! The agent's own browser (preview_* tools of the hub MCP) — a port of `browser.ts`: a
//! headless Chromium the hub starts on first use, one page per session. The agent opens the
//! app it is building, looks at screenshots, clicks, types and reads the console.
//!
//! Chromium is driven with the DevTools protocol over a pipe (`--remote-debugging-pipe`:
//! fd 3 in, fd 4 out, NUL-separated JSON), not a debugging port — no other local process can
//! take it over, and no WebSocket client is needed.
//!
//! `http://localhost:<port>` of a project session goes through the project's preview proxy:
//! the app works from a container too, and the agent sees the same page (with the preview
//! helper) as the user. The browser closes after `IDLE` without use.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use hubness_core::{BrowserResult, PreviewLog};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::config::Config;
use crate::error::{ApiResult, HttpError};
use crate::preview::{self, AGENT_UA_MARK};

const IDLE: Duration = Duration::from_secs(10 * 60);
const NAV_TIMEOUT: Duration = Duration::from_secs(20);
const CANDIDATES: [&str; 8] =
    ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable", "chrome", "brave", "brave-browser", "microsoft-edge"];

/// `POST /api/sessions/:id/browser`.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRequest {
    pub action: Action,
    pub url: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub selector: Option<String>,
    pub text: Option<String>,
    pub value: Option<String>,
    pub key: Option<String>,
    pub expression: Option<String>,
    pub full_page: Option<bool>,
    pub screenshot: Option<bool>,
}

#[derive(Deserialize, Default, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Open,
    Screenshot,
    Click,
    Type,
    Press,
    Eval,
    #[default]
    Console,
    Close,
}

impl BrowserRequest {
    /// The limits of the TypeScript server's zod schema.
    pub fn check(&self) -> ApiResult<()> {
        let long = |f: &str, v: &Option<String>, max: usize| match v {
            Some(s) if s.encode_utf16().count() > max => Err(HttpError::bad_request(format!("{f}: at most {max} characters"))),
            _ => Ok(()),
        };
        long("url", &self.url, 4000)?;
        long("selector", &self.selector, 2000)?;
        long("text", &self.text, 2000)?;
        long("value", &self.value, 100_000)?;
        long("key", &self.key, 40)?;
        long("expression", &self.expression, 100_000)?;
        if self.width.is_some_and(|w| !(240..=3840).contains(&w)) {
            return Err(HttpError::bad_request("width: 240-3840"));
        }
        if self.height.is_some_and(|h| !(240..=4000).contains(&h)) {
            return Err(HttpError::bad_request("height: 240-4000"));
        }
        Ok(())
    }
}

/// `HUBNESS_CHROME`, else the first Chromium-like browser on PATH.
pub fn find_chrome() -> Option<String> {
    if let Ok(v) = std::env::var("HUBNESS_CHROME")
        && !v.is_empty()
    {
        return Some(v);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .find_map(|d| CANDIDATES.iter().map(|n| d.join(n)).find(|p| p.is_file()).map(|p| p.to_string_lossy().into_owned()))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------- the DevTools connection ----------

type Reply = oneshot::Sender<Result<Value, String>>;

/// One DevTools connection over the pipe; commands of a page carry its sessionId.
struct Cdp {
    /// Messages for Chromium; a thread of its own writes them (the pipe blocks when Chromium
    /// stops reading, and that must never hold an async worker).
    out: Mutex<mpsc::Sender<Vec<u8>>>,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, Reply>>,
    closed: AtomicBool,
}

impl Cdp {
    async fn send(&self, method: &str, params: Value, session: Option<&str>) -> Result<Value, String> {
        self.send_for(method, params, session, NAV_TIMEOUT).await
    }

    async fn send_for(&self, method: &str, params: Value, session: Option<&str>, timeout: Duration) -> Result<Value, String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("The browser closed".into());
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        if !self.write(id, method, params, session) {
            lock(&self.pending).remove(&id);
            return Err("The browser closed".into());
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("The browser closed".into()),
            Err(_) => {
                lock(&self.pending).remove(&id);
                Err(format!("{method} timed out"))
            }
        }
    }

    /// Queue one message for Chromium.
    fn write(&self, id: u64, method: &str, params: Value, session: Option<&str>) -> bool {
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        let mut line = msg.to_string().into_bytes();
        line.push(0);
        lock(&self.out).send(line).is_ok()
    }

    /// A command nobody waits for (sent from the reader thread, which must not block).
    fn fire(&self, method: &str, params: Value, session: Option<&str>) {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        self.write(id, method, params, session);
    }

    fn fail(&self) {
        self.closed.store(true, Ordering::SeqCst);
        for (_, tx) in lock(&self.pending).drain() {
            let _ = tx.send(Err("The browser closed".into()));
        }
    }
}

/// Read NUL-separated messages from Chromium (a plain thread: the pipe is blocking).
fn read_loop(cdp: Arc<Cdp>, out: std::fs::File) {
    let mut reader = BufReader::new(out);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(0, &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buf.last() == Some(&0) {
            buf.pop();
        }
        let Ok(m) = serde_json::from_slice::<Value>(&buf) else { continue };
        if let Some(id) = m.get("id").and_then(Value::as_u64) {
            if let Some(tx) = lock(&cdp.pending).remove(&id) {
                let r = match m.get("error") {
                    Some(e) => Err(e.get("message").and_then(Value::as_str).unwrap_or("DevTools error").to_string()),
                    None => Ok(m.get("result").cloned().unwrap_or_else(|| json!({}))),
                };
                let _ = tx.send(r);
            }
        } else if let Some(method) = m.get("method").and_then(Value::as_str) {
            on_event(&cdp, method, m.get("params").unwrap_or(&Value::Null), m.get("sessionId").and_then(Value::as_str));
        }
    }
    cdp.fail();
    // the pages belonged to this browser (a newer one keeps its own)
    let current = lock(&CHROME).as_ref().is_some_and(|c| Arc::ptr_eq(&c.cdp, &cdp));
    if current {
        lock(&PAGES).clear();
    }
}

// ---------- the browser and its pages ----------

struct Chrome {
    child: Child,
    cdp: Arc<Cdp>,
    /// The tail of what the browser wrote to stderr: the only clue when it won't start.
    said: Arc<Mutex<String>>,
}

/// `HUBNESS_CHROME_ARGS`: extra arguments for the browser (`--no-sandbox` where its sandbox
/// can't start).
pub fn chrome_args(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(String::from).collect()
}

/// Why the browser didn't start, for a person: the reason, what Chromium itself said last,
/// and the way out when it is its sandbox (unprivileged user namespaces are restricted on
/// Ubuntu 23.10+, in containers, and it refuses to run as root). The same text as
/// `startupError` in browser.ts.
pub fn startup_error(reason: &str, stderr: &str) -> String {
    static NOISE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\[[^\]]*:(INFO|WARNING):").expect("a valid pattern"));
    static SANDBOX: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)sandbox|namespace|setuid|as root").expect("a valid pattern"));
    let lines: Vec<&str> =
        stderr.lines().map(str::trim).filter(|l| !l.is_empty() && !NOISE.is_match(l) && !l.contains("DevTools listening")).collect();
    let said = lines[lines.len().saturating_sub(3)..].join(" ");
    let said: String = said.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
    let mut out = format!("Chromium didn't start: {reason}");
    if !said.is_empty() {
        out.push_str(&format!(" — {said}"));
    }
    if SANDBOX.is_match(stderr) {
        out.push_str(" Its sandbox can’t start on this system: set HUBNESS_CHROME_ARGS=--no-sandbox for the hub to run the agents’ browser without it.");
    }
    out
}

struct Page {
    session_id: String,
    target_id: String,
    /// Logs of this page not yet returned to the agent.
    logs: Vec<PreviewLog>,
    seq: u64,
    /// The preview key the page is on (for the user's logs) and how far they were read.
    preview_key: Option<String>,
    user_seen: u64,
    /// localhost:<app port> ⇄ 127.0.0.1:<preview port>: the agent sees its own address.
    alias: Option<(String, String)>,
    /// The main frame is loading a document (a click or a key may have started it).
    loading: bool,
    loaded: Option<oneshot::Sender<()>>,
}

static CHROME: LazyLock<Mutex<Option<Chrome>>> = LazyLock::new(Default::default);
static STARTING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);
static PAGES: LazyLock<Mutex<HashMap<String, Page>>> = LazyLock::new(Default::default);
static LAST_USE: LazyLock<Mutex<Instant>> = LazyLock::new(|| Mutex::new(Instant::now()));

fn touch() {
    *lock(&LAST_USE) = Instant::now();
}

/// Two pipes: fd 3 of the child reads what we write, fd 4 of the child writes what we read.
fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: pipe2 fills two descriptors on success
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both descriptors are fresh and owned by nobody else
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn launch(config: &Config) -> ApiResult<Chrome> {
    let bin = find_chrome().ok_or_else(|| {
        HttpError::new(
            503,
            "No Chromium or Chrome found on the hub machine. Install chromium (or set HUBNESS_CHROME) to let agents use a browser.",
        )
    })?;
    let profile = config.data_dir.join("browser");
    std::fs::create_dir_all(&profile)?;
    let (cmd_r, cmd_w) = pipe()?;
    let (out_r, out_w) = pipe()?;
    let (a, b) = (cmd_r.as_raw_fd(), out_w.as_raw_fd());
    let mut command = Command::new(&bin);
    command
        .args([
            "--headless=new",
            "--remote-debugging-pipe",
            &format!("--user-data-dir={}", profile.display()),
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--hide-scrollbars",
            "--mute-audio",
        ])
        .args(chrome_args(&std::env::var("HUBNESS_CHROME_ARGS").unwrap_or_default()))
        .arg("about:blank")
        .env_clear()
        .envs(crate::config::child_env())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        // own process group: shutdown takes the renderers along
        .process_group(0);
    // SAFETY: only async-signal-safe calls (fcntl, dup2) between fork and exec
    unsafe {
        command.pre_exec(move || {
            // move both out of the way first, so dup2 onto 3 can't clobber the other one
            let hi_a = libc::fcntl(a, libc::F_DUPFD, 10);
            let hi_b = libc::fcntl(b, libc::F_DUPFD, 10);
            if hi_a < 0 || hi_b < 0 || libc::dup2(hi_a, 3) < 0 || libc::dup2(hi_b, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|e| HttpError::new(503, startup_error(&e.to_string(), "")))?;
    drop((cmd_r, out_w));
    // keep the tail of stderr, and always drain it (a full pipe would stall the browser)
    let said = Arc::new(Mutex::new(String::new()));
    if let Some(mut err) = child.stderr.take() {
        let said = said.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut tail = lock(&said);
                tail.push_str(&String::from_utf8_lossy(&buf[..n]));
                if tail.len() > 4000 {
                    let cut = tail.len() - 4000;
                    let cut = (cut..tail.len()).find(|i| tail.is_char_boundary(*i)).unwrap_or(tail.len());
                    tail.drain(..cut);
                }
            }
        });
    }
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let cdp = Arc::new(Cdp { out: Mutex::new(tx), next: AtomicU64::new(0), pending: Mutex::default(), closed: AtomicBool::new(false) });
    // weak: the thread must not keep the Cdp (and with it its own sender) alive
    let writer = Arc::downgrade(&cdp);
    let mut input = std::fs::File::from(cmd_w);
    std::thread::spawn(move || {
        // ends when the Cdp (the sender) is dropped, or Chromium closed its end
        for line in rx {
            if input.write_all(&line).is_err() {
                if let Some(cdp) = writer.upgrade() {
                    cdp.fail();
                }
                break;
            }
        }
    });
    let reader = cdp.clone();
    let out = std::fs::File::from(out_r);
    std::thread::spawn(move || read_loop(reader, out));
    Ok(Chrome { child, cdp, said })
}

async fn browser(config: &Config) -> ApiResult<Arc<Cdp>> {
    if let Some(c) = lock(&CHROME).as_ref().filter(|c| !c.cdp.closed.load(Ordering::SeqCst)) {
        return Ok(c.cdp.clone());
    }
    let _starting = STARTING.lock().await;
    if let Some(c) = lock(&CHROME).as_ref().filter(|c| !c.cdp.closed.load(Ordering::SeqCst)) {
        return Ok(c.cdp.clone());
    }
    shutdown();
    let chrome = launch(config)?;
    let cdp = chrome.cdp.clone();
    let said = chrome.said.clone();
    *lock(&CHROME) = Some(chrome);
    if let Err(e) = cdp.send_for("Browser.getVersion", json!({}), None, Duration::from_secs(15)).await {
        // a moment for stderr to arrive after the pipe closed
        tokio::time::sleep(Duration::from_millis(100)).await;
        let said = lock(&said).clone();
        shutdown();
        return Err(HttpError::new(503, startup_error(&e.to_string(), &said)));
    }
    touch();
    // close the browser after a while without use
    let watched = cdp.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if watched.closed.load(Ordering::SeqCst) {
                return;
            }
            if lock(&LAST_USE).elapsed() > IDLE {
                let current = lock(&CHROME).as_ref().is_some_and(|c| Arc::ptr_eq(&c.cdp, &watched));
                if current {
                    shutdown();
                }
                return;
            }
        }
    });
    Ok(cdp)
}

fn page_by_session<'a>(pages: &'a mut HashMap<String, Page>, session: Option<&str>) -> Option<&'a mut Page> {
    let s = session?;
    pages.values_mut().find(|p| p.session_id == s)
}

fn push_log(p: &mut Page, level: &str, text: &str, url: &str) {
    p.seq += 1;
    let text: String = text.chars().take(4000).collect();
    p.logs.push(PreviewLog { seq: p.seq, ts: crate::db::now(), level: level.into(), text, url: url.into(), source: "agent".into() });
    if p.logs.len() > 200 {
        let extra = p.logs.len() - 200;
        p.logs.drain(..extra);
    }
}

/// A console argument as text (a value, else its description).
fn remote(o: &Value) -> String {
    match o.get("value") {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => o.get("description").or_else(|| o.get("type")).and_then(Value::as_str).unwrap_or("").to_string(),
    }
}

fn s<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    path.iter().try_fold(v, |v, k| v.get(*k)).and_then(Value::as_str).unwrap_or("")
}

fn on_event(cdp: &Cdp, method: &str, params: &Value, session: Option<&str>) {
    let mut pages = lock(&PAGES);
    let Some(p) = page_by_session(&mut pages, session) else { return };
    match method {
        "Runtime.consoleAPICalled" => {
            let level = match s(params, &["type"]) {
                "error" | "assert" => "error",
                "warning" => "warn",
                _ => "log",
            };
            let text = params
                .get("args")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(remote).collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            let url = params.pointer("/stackTrace/callFrames/0/url").and_then(Value::as_str).unwrap_or("").to_string();
            push_log(p, level, &text, &url);
        }
        "Runtime.exceptionThrown" => {
            let d = params.get("exceptionDetails").unwrap_or(&Value::Null);
            let text = d
                .pointer("/exception/description")
                .and_then(Value::as_str)
                .or_else(|| d.get("text").and_then(Value::as_str))
                .unwrap_or("Uncaught exception");
            let url = s(d, &["url"]).to_string();
            push_log(p, "error", text, &url);
        }
        "Log.entryAdded" => {
            // network errors come from Network.* (with the address) — not twice
            let e = params.get("entry").unwrap_or(&Value::Null);
            // …and what an app's own content policy says about the preview helper is not the app's error
            if s(e, &["level"]) == "error" && s(e, &["source"]) != "network" && !s(e, &["text"]).contains("/__hubness/") {
                let (text, url) = (s(e, &["text"]).to_string(), s(e, &["url"]).to_string());
                push_log(p, "error", &text, &url);
            }
        }
        "Network.responseReceived" => {
            let r = params.get("response").unwrap_or(&Value::Null);
            let status = r.get("status").and_then(Value::as_f64).unwrap_or(0.0);
            let url = s(r, &["url"]).to_string();
            if status >= 400.0 && !url.contains("/__hubness/") {
                push_log(p, "network", &format!("{} {url}", status as i64), &url);
            }
        }
        "Network.loadingFailed" => {
            if !params.get("canceled").and_then(Value::as_bool).unwrap_or(false) {
                let t = format!("Request failed: {}", s(params, &["errorText"]));
                push_log(p, "network", &t, "");
            }
        }
        "Page.javascriptDialogOpening" => {
            // nobody is there to answer alert / confirm / prompt: say yes, or the page hangs for good
            let kind = match s(params, &["type"]) {
                "" => "dialog",
                t => t,
            };
            let message: String = s(params, &["message"]).chars().take(500).collect();
            let sid = p.session_id.clone();
            push_log(p, "log", &format!("The page showed a {kind}: “{message}” — accepted"), "");
            cdp.fire("Page.handleJavaScriptDialog", json!({ "accept": true }), Some(&sid));
        }
        // the target id of a page is the id of its main frame
        "Page.frameStartedLoading" if s(params, &["frameId"]) == p.target_id => p.loading = true,
        "Page.frameStoppedLoading" if s(params, &["frameId"]) == p.target_id => p.loading = false,
        "Page.loadEventFired" => {
            p.loading = false;
            if let Some(tx) = p.loaded.take() {
                let _ = tx.send(());
            }
        }
        _ => {}
    }
}

fn cdp_err(e: String) -> HttpError {
    HttpError::new(500, e)
}

/// The page's DevTools session id for a hub session (the page is created on first use).
async fn page_for(config: &Config, hub_session: &str) -> ApiResult<(Arc<Cdp>, String)> {
    let cdp = browser(config).await?;
    touch();
    if let Some(p) = lock(&PAGES).get(hub_session) {
        return Ok((cdp, p.session_id.clone()));
    }
    let created = cdp.send("Target.createTarget", json!({ "url": "about:blank" }), None).await.map_err(cdp_err)?;
    let target_id = s(&created, &["targetId"]).to_string();
    let attached = cdp.send("Target.attachToTarget", json!({ "targetId": target_id, "flatten": true }), None).await.map_err(cdp_err)?;
    let sid = s(&attached, &["sessionId"]).to_string();
    lock(&PAGES).insert(
        hub_session.to_string(),
        Page {
            session_id: sid.clone(),
            target_id,
            logs: vec![],
            seq: 0,
            preview_key: None,
            user_seen: 0,
            alias: None,
            loading: false,
            loaded: None,
        },
    );
    let version = cdp.send("Browser.getVersion", json!({}), None).await.map_err(cdp_err)?;
    let ua = format!("{} {AGENT_UA_MARK}", s(&version, &["userAgent"]).replace("HeadlessChrome", "Chrome"));
    let sid_ref = Some(sid.as_str());
    for (m, p) in [
        ("Page.enable", json!({})),
        ("Runtime.enable", json!({})),
        ("Log.enable", json!({})),
        ("Network.enable", json!({})),
        ("Network.setUserAgentOverride", json!({ "userAgent": ua })),
    ] {
        cdp.send(m, p, sid_ref).await.map_err(cdp_err)?;
    }
    viewport(&cdp, &sid, 1280, 800).await?;
    Ok((cdp, sid))
}

async fn viewport(cdp: &Cdp, sid: &str, width: u32, height: u32) -> ApiResult<()> {
    let w = width.clamp(240, 3840);
    let h = height.clamp(240, 4000);
    cdp.send(
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": w, "height": h, "deviceScaleFactor": 1, "mobile": w < 600 }),
        Some(sid),
    )
    .await
    .map_err(cdp_err)?;
    cdp.send("Emulation.setTouchEmulationEnabled", json!({ "enabled": w < 600 }), Some(sid)).await.map_err(cdp_err)?;
    Ok(())
}

/// Run an expression in the page; the value comes back as JSON (`None` for undefined).
async fn evaluate(cdp: &Cdp, sid: &str, expression: &str) -> ApiResult<Option<Value>> {
    let r = cdp
        .send(
            "Runtime.evaluate",
            json!({ "expression": expression, "awaitPromise": true, "returnByValue": true, "userGesture": true }),
            Some(sid),
        )
        .await
        .map_err(cdp_err)?;
    if let Some(d) = r.get("exceptionDetails") {
        let msg = d
            .pointer("/exception/description")
            .and_then(Value::as_str)
            .or_else(|| d.get("text").and_then(Value::as_str))
            .unwrap_or("The expression threw");
        return Err(HttpError::bad_request(msg.to_string()));
    }
    Ok(r.pointer("/result/value").cloned())
}

/// Where an element is (center, in CSS pixels), scrolled into view; by selector or visible text.
async fn locate(cdp: &Cdp, sid: &str, b: &BrowserRequest) -> ApiResult<(f64, f64, String)> {
    let find = match &b.selector {
        Some(sel) => format!("document.querySelector({})", json!(sel)),
        None => format!(
            "(() => {{ const want = {};
        const all = [...document.querySelectorAll('a,button,input,select,textarea,label,summary,[role=button],[role=link],[role=tab],[role=menuitem],[onclick],[tabindex]')];
        const vis = (e) => {{ const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; }};
        const txt = (e) => (e.innerText || e.value || e.getAttribute('aria-label') || e.getAttribute('placeholder') || e.title || '').trim().toLowerCase();
        return all.find((e) => vis(e) && txt(e) === want) || all.find((e) => vis(e) && txt(e).includes(want))
          || [...document.querySelectorAll('body *')].find((e) => vis(e) && e.children.length === 0 && (e.textContent || '').trim().toLowerCase().includes(want)) || null; }})()",
            json!(b.text.as_deref().unwrap_or("").trim().to_lowercase())
        ),
    };
    let expr = format!(
        "(() => {{ const e = {find}; if (!e) return null; e.scrollIntoView({{ block: 'center', inline: 'center' }});
      const r = e.getBoundingClientRect(); return {{ x: r.left + r.width / 2, y: r.top + r.height / 2,
      desc: e.tagName.toLowerCase() + (e.id ? '#' + e.id : '') + ' \"' + (e.innerText || e.value || '').trim().slice(0, 60) + '\"' }}; }})()"
    );
    let at = evaluate(cdp, sid, &expr).await?.filter(|v| v.is_object());
    let Some(at) = at else {
        return Err(HttpError::not_found(match &b.selector {
            Some(sel) => format!("No element matches {sel}"),
            None => format!("No element with the text “{}”", b.text.as_deref().unwrap_or("")),
        }));
    };
    let n = |k: &str| at.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    Ok((n("x"), n("y"), s(&at, &["desc"]).to_string()))
}

/// After a click or a key: if it started a navigation, wait for the new page; else for the
/// app to react.
async fn wait_settled(hub_session: &str, ms: u64) {
    tokio::time::sleep(Duration::from_millis(150)).await;
    let waiter = {
        let mut pages = lock(&PAGES);
        pages.get_mut(hub_session).filter(|p| p.loading).map(|p| {
            let (tx, rx) = oneshot::channel();
            p.loaded = Some(tx);
            rx
        })
    };
    let Some(rx) = waiter else { return tokio::time::sleep(Duration::from_millis(ms)).await };
    let _ = tokio::time::timeout(Duration::from_secs(10), rx).await;
    if let Some(p) = lock(&PAGES).get_mut(hub_session) {
        p.loaded = None;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// (key, code, keyCode, text) of the named keys.
const KEYS: [(&str, &str, &str, u32, Option<&str>); 14] = [
    ("Enter", "Enter", "Enter", 13, Some("\r")),
    ("Tab", "Tab", "Tab", 9, None),
    ("Escape", "Escape", "Escape", 27, None),
    ("Backspace", "Backspace", "Backspace", 8, None),
    ("Delete", "Delete", "Delete", 46, None),
    ("Space", " ", "Space", 32, Some(" ")),
    ("ArrowUp", "ArrowUp", "ArrowUp", 38, None),
    ("ArrowDown", "ArrowDown", "ArrowDown", 40, None),
    ("ArrowLeft", "ArrowLeft", "ArrowLeft", 37, None),
    ("ArrowRight", "ArrowRight", "ArrowRight", 39, None),
    ("Home", "Home", "Home", 36, None),
    ("End", "End", "End", 35, None),
    ("PageUp", "PageUp", "PageUp", 33, None),
    ("PageDown", "PageDown", "PageDown", 34, None),
];

/// Route `http://localhost:<port>` of a project session through its preview proxy: the
/// preview port for an app port.
pub type PreviewFor = Box<dyn Fn(u16) -> Option<u16> + Send + Sync>;

/// What the agent asked to open, as an address: `localhost:5173/x` has no scheme
/// ("localhost:" is not one) and gets http://. Only http and https — no file:, this browser
/// runs outside the sandbox that hides the user's files from the agent.
fn parse_target(url: &str) -> ApiResult<reqwest::Url> {
    static SCHEME: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)^([a-z][a-z0-9+.-]*://|about:)").unwrap());
    let mut target = url.trim().to_string();
    if !SCHEME.is_match(&target) {
        target = format!("http://{target}");
    }
    // an http address needs a host ("http://" alone is not one), as in the TypeScript server
    let parsed = reqwest::Url::parse(&target)
        .ok()
        .filter(|u| !matches!(u.scheme(), "http" | "https") || u.host_str().is_some_and(|h| !h.is_empty()));
    let Some(u) = parsed else { return Err(HttpError::bad_request(format!("Not a URL: {url}"))) };
    if !matches!(u.scheme(), "http" | "https") && target != "about:blank" {
        return Err(HttpError::bad_request("Only http and https addresses"));
    }
    Ok(u)
}

/// `http://localhost:<port>` (any spelling of this machine): the app port.
fn local_app_port(u: &reqwest::Url) -> Option<u16> {
    let local = matches!(u.host_str()?, "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]");
    (u.scheme() == "http" && local).then(|| u.port()).flatten()
}

/// Path, query and fragment of an address.
fn after_origin(u: &reqwest::Url) -> String {
    let mut out = u.path().to_string();
    if let Some(q) = u.query() {
        out.push('?');
        out.push_str(q);
    }
    if let Some(f) = u.fragment() {
        out.push('#');
        out.push_str(f);
    }
    out
}

async fn navigate(cdp: &Cdp, sid: &str, hub_session: &str, url: &str, preview_for: Option<&PreviewFor>) -> ApiResult<()> {
    let u = parse_target(url)?;
    let mut target = u.to_string();
    let mut alias = None;
    let mut preview_key = None;
    if let Some(pf) = preview_for
        && let Some(port) = local_app_port(&u)
        && let Some(pp) = pf(port)
    {
        // origin ⇄ the preview port; a user:password@ part is not carried over
        let from = u.origin().ascii_serialization();
        let to = format!("http://127.0.0.1:{pp}");
        target = format!("{to}{}", after_origin(&u));
        preview_key = preview::preview_key_by_port(pp);
        // a hub with accounts: the proxy of that port lets this browser in by its pass — not
        // a hub credential (see `auth::preview_pass`), and the proxy strips it again
        let pass = json!({ "name": crate::auth::PREVIEW_COOKIE, "value": crate::auth::preview_pass(pp), "url": to, "httpOnly": true });
        let _ = cdp.send("Network.setCookie", pass, Some(sid)).await;
        alias = Some((from, to));
    }
    let user_seen = preview_key.as_deref().and_then(|k| preview::preview_logs(k, 0).last().map(|l| l.seq)).unwrap_or(0);
    let (tx, rx) = oneshot::channel();
    {
        let mut pages = lock(&PAGES);
        if let Some(p) = pages.get_mut(hub_session) {
            p.alias = alias;
            p.preview_key = preview_key;
            p.user_seen = user_seen;
            p.loaded = Some(tx);
        }
    }
    let r = cdp.send("Page.navigate", json!({ "url": target }), Some(sid)).await.map_err(cdp_err)?;
    if let Some(err) = r.get("errorText").and_then(Value::as_str).filter(|e| !e.is_empty()) {
        if let Some(p) = lock(&PAGES).get_mut(hub_session) {
            p.loaded = None;
        }
        return Err(HttpError::new(502, format!("Can't open {url}: {err}")));
    }
    // no loader: the same document (only the #hash changed) — there is no load to wait for
    if r.get("loaderId").and_then(Value::as_str).is_some_and(|l| !l.is_empty()) {
        let _ = tokio::time::timeout(NAV_TIMEOUT, rx).await;
    }
    if let Some(p) = lock(&PAGES).get_mut(hub_session) {
        p.loaded = None;
    }
    // client-side apps render after load
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

async fn screenshot(cdp: &Cdp, sid: &str, full_page: bool) -> ApiResult<String> {
    let mut params = json!({ "format": "png" });
    if full_page {
        let m = cdp.send("Page.getLayoutMetrics", json!({}), Some(sid)).await.map_err(cdp_err)?;
        let size = m.get("cssContentSize").or_else(|| m.get("contentSize")).cloned().unwrap_or(Value::Null);
        let n = |k: &str| size.get(k).and_then(Value::as_f64).unwrap_or(0.0).ceil();
        params["clip"] = json!({ "x": 0, "y": 0, "width": n("width").min(3840.0), "height": n("height").min(8000.0), "scale": 1 });
        params["captureBeyondViewport"] = json!(true);
    }
    let r = cdp.send("Page.captureScreenshot", params, Some(sid)).await.map_err(cdp_err)?;
    Ok(s(&r, &["data"]).to_string())
}

fn result(text: &str) -> BrowserResult {
    BrowserResult { ok: true, url: String::new(), title: String::new(), text: text.into(), screenshot: None, logs: vec![] }
}

/// Actions of one session run one after another: a page has one address, one pending load.
static QUEUES: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Default::default);

/// One agent browser action for a hub session.
pub async fn browser_action(
    config: &Config,
    hub_session: &str,
    b: BrowserRequest,
    preview_for: Option<PreviewFor>,
) -> ApiResult<BrowserResult> {
    b.check()?;
    let queue = lock(&QUEUES).entry(hub_session.to_string()).or_default().clone();
    let result = {
        let _turn = queue.lock().await;
        act(config, hub_session, b, preview_for).await
    };
    // nobody else is waiting: forget the queue (2 = this clone and the map's)
    let mut queues = lock(&QUEUES);
    if Arc::strong_count(&queue) <= 2 {
        queues.remove(hub_session);
    }
    result
}

async fn act(config: &Config, hub_session: &str, b: BrowserRequest, preview_for: Option<PreviewFor>) -> ApiResult<BrowserResult> {
    if b.action == Action::Close {
        close_session_browser(hub_session).await;
        return Ok(result("Browser page closed"));
    }
    if b.action == Action::Console && !lock(&PAGES).contains_key(hub_session) {
        return Ok(result("No page open yet — use preview_open first"));
    }
    let (cdp, sid) = page_for(config, hub_session).await?;
    let mut shot = None;
    let text = match b.action {
        Action::Open => {
            let Some(url) = b.url.as_deref() else { return Err(HttpError::bad_request("url is required")) };
            if b.width.is_some() || b.height.is_some() {
                viewport(&cdp, &sid, b.width.unwrap_or(1280), b.height.unwrap_or(800)).await?;
            }
            navigate(&cdp, &sid, hub_session, url, preview_for.as_ref()).await?;
            "Opened".to_string()
        }
        Action::Screenshot => {
            shot = Some(screenshot(&cdp, &sid, b.full_page.unwrap_or(false)).await?);
            "Screenshot taken".into()
        }
        Action::Click => {
            if b.selector.is_none() && b.text.is_none() {
                return Err(HttpError::bad_request("selector or text is required"));
            }
            let (x, y, desc) = locate(&cdp, &sid, &b).await?;
            for t in ["mouseMoved", "mousePressed", "mouseReleased"] {
                let clicks = if t == "mouseMoved" { 0 } else { 1 };
                cdp.send(
                    "Input.dispatchMouseEvent",
                    json!({ "type": t, "x": x, "y": y, "button": "left", "clickCount": clicks }),
                    Some(&sid),
                )
                .await
                .map_err(cdp_err)?;
            }
            wait_settled(hub_session, 600).await;
            format!("Clicked {desc}")
        }
        Action::Type => {
            if b.selector.is_none() && b.text.is_none() {
                return Err(HttpError::bad_request("selector or text is required"));
            }
            let (x, y, desc) = locate(&cdp, &sid, &b).await?;
            for t in ["mousePressed", "mouseReleased"] {
                cdp.send("Input.dispatchMouseEvent", json!({ "type": t, "x": x, "y": y, "button": "left", "clickCount": 1 }), Some(&sid))
                    .await
                    .map_err(cdp_err)?;
            }
            // replace the value: select everything in the focused field, then type over it
            evaluate(&cdp, &sid, "(() => { const e = document.activeElement; if (e && 'select' in e) e.select(); else document.execCommand('selectAll'); })()")
                .await?;
            cdp.send("Input.insertText", json!({ "text": b.value.as_deref().unwrap_or("") }), Some(&sid)).await.map_err(cdp_err)?;
            wait_settled(hub_session, 300).await;
            format!("Typed into {desc}")
        }
        Action::Press => {
            let name = b.key.as_deref().unwrap_or("");
            let named = KEYS.iter().find(|k| k.0 == name).map(|k| (k.1.to_string(), k.2.to_string(), k.3, k.4.map(String::from)));
            let single = (name.encode_utf16().count() == 1).then(|| {
                let code = name.to_uppercase().encode_utf16().next().map(u32::from).unwrap_or(0);
                (name.to_string(), String::new(), code, Some(name.to_string()))
            });
            let Some((key, code, key_code, text)) = named.or(single) else {
                let names: Vec<&str> = KEYS.iter().map(|k| k.0).collect();
                return Err(HttpError::bad_request(format!("Unknown key {name}; use {} or one character", names.join(", "))));
            };
            let mut down = json!({ "type": if text.is_some() { "keyDown" } else { "rawKeyDown" }, "key": key, "code": code, "windowsVirtualKeyCode": key_code });
            if let Some(t) = &text {
                down["text"] = json!(t);
            }
            cdp.send("Input.dispatchKeyEvent", down, Some(&sid)).await.map_err(cdp_err)?;
            cdp.send(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": key_code }),
                Some(&sid),
            )
            .await
            .map_err(cdp_err)?;
            wait_settled(hub_session, 600).await;
            format!("Pressed {key}")
        }
        Action::Eval => {
            let Some(expr) = b.expression.as_deref().filter(|e| !e.is_empty()) else {
                return Err(HttpError::bad_request("expression is required"));
            };
            match evaluate(&cdp, &sid, expr).await? {
                None => "undefined".into(),
                Some(v) => serde_json::to_string_pretty(&v).unwrap_or_default().chars().take(20_000).collect(),
            }
        }
        Action::Console => "Console".into(),
        Action::Close => unreachable!(),
    };
    if b.screenshot == Some(true) && shot.is_none() {
        shot = Some(screenshot(&cdp, &sid, false).await?);
    }
    let info = evaluate(&cdp, &sid, "({ url: location.href, title: document.title })").await.ok().flatten().unwrap_or(Value::Null);
    let (url, title) = (s(&info, &["url"]).to_string(), s(&info, &["title"]).to_string());
    let (logs, shown) = {
        let mut pages = lock(&PAGES);
        match pages.get_mut(hub_session) {
            Some(p) => {
                let mut logs: Vec<PreviewLog> = std::mem::take(&mut p.logs);
                if let Some(key) = &p.preview_key {
                    let user: Vec<PreviewLog> =
                        preview::preview_logs(key, p.user_seen).into_iter().filter(|l| l.source == "user").collect();
                    if let Some(last) = user.last() {
                        p.user_seen = last.seq;
                    }
                    let skip = user.len().saturating_sub(50);
                    logs.extend(user.into_iter().skip(skip));
                }
                // the page's address as the agent wrote it (the preview proxy port mapped back)
                let shown = match &p.alias {
                    Some((from, to)) if url.starts_with(to.as_str()) => format!("{from}{}", &url[to.len()..]),
                    _ => url,
                };
                (logs, shown)
            }
            None => (vec![], url),
        }
    };
    touch();
    Ok(BrowserResult { ok: true, url: shown, title, text, screenshot: shot, logs })
}

pub async fn close_session_browser(hub_session: &str) {
    let Some(p) = lock(&PAGES).remove(hub_session) else { return };
    let cdp = lock(&CHROME).as_ref().map(|c| c.cdp.clone());
    if let Some(cdp) = cdp {
        let _ = cdp.send("Target.closeTarget", json!({ "targetId": p.target_id }), None).await;
    }
}

/// How many pages the agents have open (tests).
pub fn open_pages() -> usize {
    lock(&PAGES).len()
}

/// Stop the browser (hub exit, idle): the whole process group, reaped.
pub fn shutdown() {
    lock(&PAGES).clear();
    let Some(mut c) = lock(&CHROME).take() else { return };
    c.cdp.fail();
    let pid = c.child.id() as i32;
    // SAFETY: signalling our own child's process group
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    std::thread::spawn(move || {
        for _ in 0..30 {
            if matches!(c.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // SAFETY: as above
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
        let _ = c.child.wait();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_like_the_zod_schema() {
        let ok = BrowserRequest { action: Action::Open, width: Some(390), ..Default::default() };
        assert!(ok.check().is_ok());
        assert!(BrowserRequest { width: Some(100), ..Default::default() }.check().is_err());
        assert!(BrowserRequest { key: Some("x".repeat(41)), ..Default::default() }.check().is_err());
        assert!(serde_json::from_str::<BrowserRequest>(r#"{"action":"fly"}"#).is_err());
        assert!(serde_json::from_str::<BrowserRequest>(r#"{"action":"open","url":"x"}"#).is_ok());
    }

    #[test]
    fn addresses_the_agent_may_open() {
        let ok = |u: &str| parse_target(u).unwrap().to_string();
        assert_eq!(ok("localhost:5173/cart"), "http://localhost:5173/cart", "no scheme: http");
        assert_eq!(ok(" https://example.com/a?b=1#c "), "https://example.com/a?b=1#c");
        assert_eq!(ok("about:blank"), "about:blank");
        for bad in ["file:///etc/passwd", "chrome://version", "javascript:alert(1)", "http://", "about:config", "data:text/html,x", ""] {
            let e = parse_target(bad).unwrap_err();
            assert_eq!(e.status.as_u16(), 400, "{bad}");
        }
        assert_eq!(parse_target("http://").unwrap_err().message, "Not a URL: http://");
        assert_eq!(parse_target("file:///etc/passwd").unwrap_err().message, "Only http and https addresses");

        // this machine's ports go through the preview; the rest of the address is kept
        let port = |u: &str| local_app_port(&parse_target(u).unwrap());
        assert_eq!(port("http://localhost:5173/x"), Some(5173));
        assert_eq!(port("127.0.0.1:3000"), Some(3000));
        assert_eq!(port("http://[::1]:8080/"), Some(8080));
        assert_eq!(port("http://0.0.0.0:8080/"), Some(8080));
        assert_eq!(port("http://localhost/"), None, "no port named");
        assert_eq!(port("https://localhost:5173/"), None);
        assert_eq!(port("http://example.com:5173/"), None);
        let u = parse_target("http://user:pw@localhost:5173/a/b?c=d#e").unwrap();
        assert_eq!(u.origin().ascii_serialization(), "http://localhost:5173", "the user:password part is not part of the origin");
        assert_eq!(after_origin(&u), "/a/b?c=d#e");
        assert_eq!(after_origin(&parse_target("localhost:5173").unwrap()), "/");
    }

    #[test]
    fn a_failed_start_says_why_and_what_to_do() {
        assert_eq!(startup_error("The browser closed", ""), "Chromium didn't start: The browser closed");
        let ubuntu = "[1:1:1128/1.0:WARNING:x.cc(1)] noise\n[2:2:1128/1.0:FATAL:zygote_host_impl_linux.cc(132)] No usable sandbox! If you are running on Ubuntu 23.10+ …\n";
        let msg = startup_error("The browser closed", ubuntu);
        assert!(
            msg.starts_with(
                "Chromium didn't start: The browser closed — [2:2:1128/1.0:FATAL:zygote_host_impl_linux.cc(132)] No usable sandbox!"
            ),
            "{msg}"
        );
        assert!(!msg.contains("noise"), "warnings are not the reason");
        assert!(msg.ends_with("set HUBNESS_CHROME_ARGS=--no-sandbox for the hub to run the agents’ browser without it."));
        let other =
            startup_error("The browser closed", "error while loading shared libraries: libnss3.so: cannot open shared object file\n");
        assert!(other.contains("libnss3.so") && !other.contains("HUBNESS_CHROME_ARGS"), "another cause gets no sandbox advice");
        assert!(startup_error("x", &"é".repeat(5000)).chars().count() < 700);
        assert_eq!(chrome_args("  --no-sandbox   --disable-foo=1 "), ["--no-sandbox", "--disable-foo=1"]);
        assert!(chrome_args("").is_empty());
    }
}
