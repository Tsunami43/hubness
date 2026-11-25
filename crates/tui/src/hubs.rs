//! Saved hubs and turning a typed address into a hub URL.
//!
//! `$XDG_CONFIG_HOME/hubness/tui.json` (default `~/.config/hubness/tui.json`, readable by the
//! owner only):
//!
//! ```json
//! { "last": "https://hub.example.com",
//!   "hubs": [ { "url": "https://hub.example.com", "username": "alice", "token": "…" } ] }
//! ```
//!
//! The first versions kept `"hubs": { "<url>": { "token", "user" } }`; it is read and
//! rewritten in the new shape on the next save.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

/// A saved hub.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hub {
    pub url: String,
    pub username: Option<String>,
    pub token: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Store {
    pub last: Option<String>,
    pub hubs: Vec<Hub>,
}

/// `$XDG_CONFIG_HOME/hubness/tui.json` (default `~/.config/hubness/tui.json`).
fn store_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("hubness").join("tui.json"))
}

pub fn key(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn str_of(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(String::from)
}

impl Store {
    pub fn load() -> Self {
        store_path().map(|p| Self::load_from(&p)).unwrap_or_default()
    }

    fn load_from(path: &Path) -> Self {
        let v: Value = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        let hubs = match &v["hubs"] {
            Value::Array(list) => list
                .iter()
                .filter_map(|h| Some(Hub { url: key(h["url"].as_str()?), username: str_of(&h["username"]), token: str_of(&h["token"]) }))
                .collect(),
            // the first format: { "<url>": { "token", "user" } }
            Value::Object(map) => {
                map.iter().map(|(url, h)| Hub { url: key(url), username: str_of(&h["user"]), token: str_of(&h["token"]) }).collect()
            }
            _ => vec![],
        };
        Self { last: str_of(&v["last"]).map(|u| key(&u)), hubs }
    }

    pub fn save(&self) {
        if let Some(path) = store_path() {
            let _ = self.save_to(&path);
        }
    }

    /// Write the file for the owner only (0600, folder 0700), replacing it atomically.
    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        if let Some(dir) = path.parent() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
        let hubs: Vec<Value> = self.hubs.iter().map(|h| json!({ "url": h.url, "username": h.username, "token": h.token })).collect();
        let v = json!({ "last": self.last, "hubs": hubs });
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(serde_json::to_string_pretty(&v).unwrap_or_default().as_bytes())?;
        f.sync_all()?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&tmp, path)
    }

    pub fn get(&self, url: &str) -> Option<&Hub> {
        let url = key(url);
        self.hubs.iter().find(|h| h.url == url)
    }

    fn entry(&mut self, url: &str) -> &mut Hub {
        let url = key(url);
        match self.hubs.iter().position(|h| h.url == url) {
            Some(i) => &mut self.hubs[i],
            None => {
                self.hubs.push(Hub { url, ..Default::default() });
                self.hubs.last_mut().expect("just pushed")
            }
        }
    }

    /// Remember a hub as the one to open next time.
    pub fn touch(&mut self, url: &str) {
        self.entry(url);
        self.last = Some(key(url));
    }

    /// The session of a hub (`token: None` signs out but keeps the username).
    pub fn set_session(&mut self, url: &str, token: Option<&str>, user: Option<&str>) {
        let hub = self.entry(url);
        hub.token = token.filter(|t| !t.is_empty()).map(String::from);
        if let Some(u) = user {
            hub.username = Some(u.to_string());
        }
    }

    pub fn forget(&mut self, url: &str) {
        let url = key(url);
        self.hubs.retain(|h| h.url != url);
        if self.last.as_deref() == Some(url.as_str()) {
            self.last = None;
        }
    }
}

/// The stored session of a hub: (token, username).
pub fn stored(url: &str) -> (Option<String>, Option<String>) {
    Store::load().get(url).map(|h| (h.token.clone(), h.username.clone())).unwrap_or_default()
}

/// Remember the session of a hub and open it next time.
pub fn remember(url: &str, token: Option<&str>, user: Option<&str>) {
    let mut s = Store::load();
    s.set_session(url, token, user);
    s.touch(url);
    s.save();
}

// ---------- addresses ----------

/// The default port of a hub.
pub const PORT: u16 = 8787;

/// URLs to try for a typed address, best first:
/// - with a scheme: as typed;
/// - `host`: `https://host` (a hub behind a reverse proxy), then `http://host:8787`;
/// - `host:port`: `https://host:port`, then `http://host:port`;
/// - nothing: this machine (`http://127.0.0.1:8787`).
pub fn candidates(input: &str) -> Vec<String> {
    let s = input.trim().trim_end_matches('/');
    if s.is_empty() {
        return vec![format!("http://127.0.0.1:{PORT}")];
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        return vec![s.to_string()];
    }
    // `host/path` would be odd for a hub; keep the path anyway
    let (authority, path) = match s.find('/') {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let has_port = if authority.starts_with('[') {
        authority.contains("]:")
    } else {
        // a bare IPv6 address has several colons and no port
        authority.matches(':').count() == 1
    };
    let authority =
        if !authority.starts_with('[') && authority.matches(':').count() > 1 { format!("[{authority}]") } else { authority.to_string() };
    if has_port {
        vec![format!("https://{authority}{path}"), format!("http://{authority}{path}")]
    } else {
        vec![format!("https://{authority}{path}"), format!("http://{authority}:{PORT}{path}")]
    }
}

/// Does a hub answer at `url` (`GET /api/health` → `{"ok": true}`)?
pub async fn probe(url: &str) -> anyhow::Result<()> {
    let client = hubness_client::Client::new(url, None)?;
    let v = tokio::time::timeout(Duration::from_secs(6), client.get::<Value>("/health"))
        .await
        .map_err(|_| anyhow::anyhow!("no answer in 6 s"))??;
    if v["ok"] != true {
        anyhow::bail!("not a hubness hub");
    }
    Ok(())
}

/// The first candidate URL of `input` where a hub answers, or the last error.
pub async fn resolve(input: &str) -> anyhow::Result<String> {
    let mut errors = vec![];
    for url in candidates(input) {
        match probe(&url).await {
            Ok(()) => return Ok(url),
            Err(e) => errors.push(format!("{url}: {}", short_error(&e))),
        }
    }
    anyhow::bail!("no hub answers — {}", errors.join("; "))
}

/// The root cause of a connection error, without reqwest's long chain.
pub fn short_error(e: &anyhow::Error) -> String {
    let root = e.chain().last().map(|c| c.to_string()).unwrap_or_else(|| e.to_string());
    let lower = root.to_lowercase();
    if lower.contains("connection refused") {
        "connection refused".into()
    } else if lower.contains("dns error") || lower.contains("failed to lookup") || lower.contains("name or service not known") {
        "unknown host".into()
    } else if lower.contains("timed out") || lower.contains("no answer") {
        "no answer".into()
    } else {
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_to_urls() {
        assert_eq!(candidates(""), ["http://127.0.0.1:8787"]);
        assert_eq!(candidates("hub.example.com"), ["https://hub.example.com", "http://hub.example.com:8787"]);
        assert_eq!(candidates(" 10.0.0.5:9000/ "), ["https://10.0.0.5:9000", "http://10.0.0.5:9000"]);
        assert_eq!(candidates("http://x:1/"), ["http://x:1"]);
        assert_eq!(candidates("::1"), ["https://[::1]", "http://[::1]:8787"]);
        assert_eq!(candidates("[::1]:8787"), ["https://[::1]:8787", "http://[::1]:8787"]);
    }

    #[test]
    fn hubs_are_stored_for_the_owner_only_and_the_old_format_is_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("hubness-tui-hubs-{}", std::process::id()));
        let path = dir.join("hubness").join("tui.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"hubs":{"http://a:1/":{"token":"tok","user":"alice"},"http://b:2":{"token":null,"user":null}}}"#)
            .unwrap();
        let mut s = Store::load_from(&path);
        assert_eq!(s.get("http://a:1").unwrap().token.as_deref(), Some("tok"));
        assert_eq!(s.get("http://a:1").unwrap().username.as_deref(), Some("alice"));
        assert!(s.last.is_none());

        s.set_session("http://a:1", None, None);
        s.touch("https://c.example");
        s.set_session("https://c.example", Some("t2"), Some("bob"));
        s.save_to(&path).unwrap();
        let back = Store::load_from(&path);
        assert_eq!(back.get("http://a:1").unwrap().token, None);
        assert_eq!(back.get("http://a:1").unwrap().username.as_deref(), Some("alice"), "signing out keeps the name");
        assert_eq!(back.last.as_deref(), Some("https://c.example"));
        assert_eq!(back.hubs.len(), 3);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

        let mut back = back;
        back.forget("https://c.example/");
        assert!(back.last.is_none() && back.get("https://c.example").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
