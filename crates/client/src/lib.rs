//! HTTP + SSE client of the hubness API, for Rust clients (the TUI). Works with either
//! server implementation: it only speaks the public `/api` contract.

use std::time::Duration;

use anyhow::{Context, bail};
use futures_util::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use hubness_core::{
    AgentConfig, ApiError, AuthStatus, GlobalStreamMsg, LoginResponse, PermissionMode, Project, Session, SessionStreamMsg, User,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc;

pub use reqwest::Method;

#[derive(Clone)]
pub struct Client {
    base: String,
    http: reqwest::Client,
    /// The session token of a hub with accounts (sent as `Authorization: Bearer`).
    token: Arc<RwLock<Option<String>>>,
    /// Set when the hub answered 401: the session is gone, sign in again.
    unauthorized: Arc<AtomicBool>,
}

impl Client {
    /// `base` is the hub URL, e.g. `http://127.0.0.1:8787`; `token` — a session token from
    /// `login` when the hub has accounts.
    pub fn new(base: &str, token: Option<&str>) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(5)).user_agent("hubness-tui").build()?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            http,
            token: Arc::new(RwLock::new(token.filter(|t| !t.is_empty()).map(String::from))),
            unauthorized: Arc::default(),
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Use another session token (`None` — signed out). Clones share it.
    pub fn set_token(&self, token: Option<String>) {
        *self.token.write().unwrap_or_else(|e| e.into_inner()) = token;
        self.unauthorized.store(false, Ordering::SeqCst);
    }

    pub fn token(&self) -> Option<String> {
        self.token.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Did a request get a 401 since the last call (the session expired or was revoked)?
    pub fn take_unauthorized(&self) -> bool {
        self.unauthorized.swap(false, Ordering::SeqCst)
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api{path}", self.base)
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let req = self.http.request(method, self.url(path));
        match self.token() {
            Some(t) => match reqwest::header::HeaderValue::from_str(&format!("Bearer {t}")) {
                Ok(mut v) => {
                    v.set_sensitive(true);
                    req.header(reqwest::header::AUTHORIZATION, v)
                }
                Err(_) => req,
            },
            None => req,
        }
    }

    async fn decode<T: DeserializeOwned>(&self, res: reqwest::Response) -> anyhow::Result<T> {
        if res.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.unauthorized.store(true, Ordering::SeqCst);
        }
        Self::decode_plain(res).await
    }

    async fn decode_plain<T: DeserializeOwned>(res: reqwest::Response) -> anyhow::Result<T> {
        let status = res.status();
        let text = res.text().await?;
        if !status.is_success() {
            let msg = serde_json::from_str::<ApiError>(&text).map(|e| e.error).unwrap_or(text);
            bail!("{}: {msg}", status.as_u16());
        }
        serde_json::from_str(&text).with_context(|| format!("unexpected response: {text}"))
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        self.decode(self.request(reqwest::Method::GET, path).send().await?).await
    }

    /// A mutating request: always JSON, which also satisfies the hub's CSRF guard.
    pub async fn send<T: DeserializeOwned, B: Serialize>(&self, method: reqwest::Method, path: &str, body: &B) -> anyhow::Result<T> {
        self.decode(self.request(method, path).json(body).send().await?).await
    }

    pub async fn health(&self) -> anyhow::Result<()> {
        self.get::<Value>("/health").await.map(|_| ())
    }

    /// Does the hub ask for a sign-in, and who is signed in with the current token?
    pub async fn auth_status(&self) -> anyhow::Result<AuthStatus> {
        Self::decode_plain(self.request(reqwest::Method::GET, "/auth/status").send().await?).await
    }

    /// Sign in; on success the client uses the new session token (also returned).
    pub async fn login(&self, username: &str, password: &str) -> anyhow::Result<LoginResponse> {
        let body = json!({ "username": username, "password": password });
        let res: LoginResponse = Self::decode_plain(self.http.post(self.url("/auth/login")).json(&body).send().await?).await?;
        self.set_token(Some(res.token.clone()));
        Ok(res)
    }

    /// End the session on the hub and forget the token.
    pub async fn logout(&self) -> anyhow::Result<()> {
        let r = self.send::<Value, _>(reqwest::Method::POST, "/auth/logout", &json!({})).await.map(|_| ());
        self.set_token(None);
        r
    }

    pub async fn users(&self) -> anyhow::Result<Vec<User>> {
        self.get("/users").await
    }

    pub async fn projects(&self) -> anyhow::Result<Vec<Project>> {
        self.get("/projects").await
    }

    /// Every active session (chats, project and group sessions), newest first.
    pub async fn sessions(&self) -> anyhow::Result<Vec<Session>> {
        self.get("/sessions?all=1").await
    }

    pub async fn default_agent(&self) -> anyhow::Result<String> {
        let v: Value = self.get("/agents").await?;
        Ok(v["defaultKind"].as_str().unwrap_or("claude-code").to_string())
    }

    /// A new chat (or project session) with the agent; `prompt` starts the first turn.
    pub async fn create_session(
        &self,
        project_id: Option<&str>,
        agent: &str,
        mode: Option<PermissionMode>,
        prompt: Option<&str>,
    ) -> anyhow::Result<Session> {
        let agent = AgentConfig { kind: agent.to_string(), model: None, permission_mode: mode, account: None };
        let body = json!({ "projectId": project_id, "agent": agent, "prompt": prompt });
        self.send(reqwest::Method::POST, "/sessions", &body).await
    }

    pub async fn send_message(&self, session_id: &str, text: &str) -> anyhow::Result<Session> {
        self.send(reqwest::Method::POST, &format!("/sessions/{session_id}/messages"), &json!({ "text": text })).await
    }

    pub async fn stop(&self, session_id: &str) -> anyhow::Result<()> {
        self.send::<Value, _>(reqwest::Method::POST, &format!("/sessions/{session_id}/stop"), &json!({})).await.map(|_| ())
    }

    pub async fn approve(&self, session_id: &str, request_id: &str, allow: bool) -> anyhow::Result<()> {
        let body = json!({ "decision": if allow { "allow" } else { "deny" } });
        self.send::<Value, _>(reqwest::Method::POST, &format!("/sessions/{session_id}/approvals/{request_id}"), &body).await.map(|_| ())
    }

    /// Attach a file to the next message of a session (raw bytes; the name in X-Filename).
    pub async fn upload(&self, session_id: &str, name: &str, mime: &str, bytes: Vec<u8>) -> anyhow::Result<Value> {
        let encoded: String = name
            .bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect();
        let res = self
            .request(reqwest::Method::POST, &format!("/sessions/{session_id}/attachments"))
            .header("X-Hubness-Request", "1")
            .header("Content-Type", mime)
            .header("X-Filename", encoded)
            .body(bytes)
            .send()
            .await?;
        self.decode(res).await
    }

    pub async fn archive(&self, session_id: &str) -> anyhow::Result<Session> {
        self.send(reqwest::Method::POST, &format!("/sessions/{session_id}/archive"), &json!({})).await
    }

    /// Subscribe to an SSE stream: messages arrive on the returned channel until the stream
    /// ends or the receiver is dropped.
    pub fn stream<T: DeserializeOwned + Send + 'static>(&self, path: &str) -> mpsc::UnboundedReceiver<anyhow::Result<T>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let req = self.request(reqwest::Method::GET, path);
        let unauthorized = self.unauthorized.clone();
        tokio::spawn(async move {
            let res = match req.send().await {
                Ok(r) if r.status().is_success() => r,
                Ok(r) => {
                    if r.status() == reqwest::StatusCode::UNAUTHORIZED {
                        unauthorized.store(true, Ordering::SeqCst);
                    }
                    let _ = tx.send(Err(anyhow::anyhow!("stream answered {}", r.status())));
                    return;
                }
                Err(e) => {
                    let _ = tx.send(Err(e.into()));
                    return;
                }
            };
            let mut body = res.bytes_stream();
            let mut buf = String::new();
            while let Some(chunk) = body.next().await {
                let Ok(chunk) = chunk else { break };
                buf.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(end) = buf.find("\n\n") {
                    let event: String = buf.drain(..end + 2).collect();
                    if let Some(msg) = parse_event(&event)
                        && tx.send(serde_json::from_str::<T>(&msg).map_err(Into::into)).is_err()
                    {
                        return;
                    }
                }
            }
        });
        rx
    }

    pub fn session_stream(&self, session_id: &str) -> mpsc::UnboundedReceiver<anyhow::Result<SessionStreamMsg>> {
        self.stream(&format!("/sessions/{session_id}/stream"))
    }

    pub fn global_stream(&self) -> mpsc::UnboundedReceiver<anyhow::Result<GlobalStreamMsg>> {
        self.stream("/stream")
    }
}

/// The `data:` payload of one SSE event; `None` for keep-alives and named events (ping).
fn parse_event(event: &str) -> Option<String> {
    let mut data = vec![];
    for line in event.lines() {
        if line.starts_with("event:") {
            return None;
        }
        if let Some(d) = line.strip_prefix("data:") {
            data.push(d.strip_prefix(' ').unwrap_or(d));
        }
    }
    let joined = data.join("\n");
    (!joined.is_empty()).then_some(joined)
}

#[cfg(test)]
mod tests {
    use super::parse_event;

    #[test]
    fn sse_events() {
        assert_eq!(parse_event("data: {\"a\":1}\n\n").as_deref(), Some("{\"a\":1}"));
        assert_eq!(parse_event("event: ping\ndata: \n\n"), None);
        assert_eq!(parse_event(": comment\n\n"), None);
        assert_eq!(parse_event("data: a\ndata: b\n\n").as_deref(), Some("a\nb"));
    }
}
