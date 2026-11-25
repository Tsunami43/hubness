//! Git logins per host — a port of `git-auth.ts` and `git-credential-helper.ts`.
//!
//! Logins (token or login + password over https) are used for the hub's own git runs:
//! fetch / pull / push from the Git panel and clone. The secret is encrypted like project
//! secrets (`secrets.rs`, same rows as the TS server) and is handed to git through our
//! credential helper (`hubcli git-credential-helper`) via the environment of that one git
//! process — never via argv. Agents never get it: their processes are started elsewhere
//! and the sandbox hides git credentials.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::LazyLock;

use hubness_core::{GitAuthProblem, GitAuthProtocol, GitAuthReason, GitCredentialInfo, GitCredentialKind};
use regex::Regex;
use rusqlite::Connection;

use crate::db::now;
use crate::error::{ApiResult, HttpError};
use crate::git_providers::parse_remote;
use crate::secrets;

/// AAD namespace (the "project id" of `secrets::encrypt`): a row copied to another host won't decrypt.
const NS: &str = "git-credential";
/// Environment variable the credential helper reads the logins from.
pub const CREDENTIALS_ENV: &str = "HUB_GIT_CREDENTIALS";
/// Subcommand of the `hubcli` binary that runs the credential helper.
pub const HELPER_COMMAND: &str = "git-credential-helper";

static HOST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$").unwrap());
static SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z]+://").unwrap());
static USERINFO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[^@/]*@").unwrap());
static PORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":[0-9]+$").unwrap());

/// `"https://user@GitLab.example.com:8443/x"` or `"gitlab.example.com"` → `"gitlab.example.com"`.
pub fn normalize_host(input: &str) -> Option<String> {
    let h = crate::git_providers::js_trim(input).to_lowercase();
    let h = SCHEME.replace(&h, "");
    let h = USERINFO.replace(&h, "");
    let first = h.split('/').next().unwrap_or("");
    let h = PORT.replace(first, "");
    (HOST_RE.is_match(&h) && h.len() <= 253).then(|| h.into_owned())
}

struct Row {
    host: String,
    username: String,
    kind: String,
    secret: String,
    updated_at: String,
}

fn rows(db: &Connection) -> rusqlite::Result<Vec<Row>> {
    let mut st = db.prepare("SELECT host, username, kind, secret, updated_at FROM git_credentials ORDER BY host")?;
    st.query_map([], |r| Ok(Row { host: r.get(0)?, username: r.get(1)?, kind: r.get(2)?, secret: r.get(3)?, updated_at: r.get(4)? }))?
        .collect()
}

pub fn list(db: &Connection) -> ApiResult<Vec<GitCredentialInfo>> {
    Ok(rows(db)?
        .into_iter()
        .map(|r| GitCredentialInfo {
            host: r.host,
            username: r.username,
            kind: if r.kind == "password" { GitCredentialKind::Password } else { GitCredentialKind::Token },
            updated_at: r.updated_at,
        })
        .collect())
}

/// The credential protocol is line-based: no newlines or NUL inside values.
fn clean(s: &str) -> bool {
    !s.contains(['\r', '\n', '\0'])
}

pub fn set(
    db: &Connection,
    data_dir: &Path,
    host_input: &str,
    username: &str,
    kind: GitCredentialKind,
    secret: &str,
) -> ApiResult<Vec<GitCredentialInfo>> {
    let host = normalize_host(host_input).ok_or_else(|| HttpError::bad_request("Host: a domain name like gitlab.example.com"))?;
    let username = crate::git_providers::js_trim(username);
    if username.is_empty() || username.encode_utf16().count() > 256 || !clean(username) {
        return Err(HttpError::bad_request("Username is required"));
    }
    if secret.is_empty() || secret.encode_utf16().count() > 4096 || !clean(secret) {
        return Err(HttpError::bad_request(if kind == GitCredentialKind::Token { "Token is required" } else { "Password is required" }));
    }
    let blob = secrets::encrypt(&secrets::keys(data_dir)?, NS, &host, secret)?;
    db.execute(
        "INSERT INTO git_credentials (host, username, kind, secret, updated_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(host) DO UPDATE SET username = excluded.username, kind = excluded.kind, secret = excluded.secret, updated_at = excluded.updated_at",
        (&host, username, kind.as_str(), blob, now()),
    )?;
    list(db)
}

pub fn delete(db: &Connection, host_input: &str) -> ApiResult<Vec<GitCredentialInfo>> {
    if let Some(host) = normalize_host(host_input) {
        db.execute("DELETE FROM git_credentials WHERE host = ?", [host])?;
    }
    list(db)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Login {
    username: String,
    password: String,
}

/// Decrypted logins for these hosts (rows that can't be decrypted are skipped).
fn credentials_for(db: &Connection, data_dir: &Path, hosts: &[String]) -> ApiResult<BTreeMap<String, Login>> {
    let mut out = BTreeMap::new();
    let wanted: Vec<Row> = rows(db)?.into_iter().filter(|r| hosts.contains(&r.host)).collect();
    if wanted.is_empty() {
        return Ok(out);
    }
    let keys = secrets::keys(data_dir)?;
    for r in wanted {
        // secret key rotated without the old one: behave as if nothing is stored
        if let Ok(password) = secrets::decrypt(&keys, NS, &r.host, &r.secret) {
            out.insert(r.host, Login { username: r.username, password });
        }
    }
    Ok(out)
}

/// `-c` options and environment that make one git run use stored logins.
#[derive(Default, Debug)]
pub struct GitAuth {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `-c` options and environment that make git use the stored logins for these remote
/// URLs. The first empty `credential.helper` clears helpers from every config level
/// (including the repository's own), so the login isn't also passed to them on
/// `store`. Nothing is returned when no login is stored for these hosts, and git
/// keeps the user's own helpers.
pub fn git_auth_for(db: &Connection, data_dir: &Path, urls: &[String]) -> ApiResult<GitAuth> {
    let hosts: Vec<String> = urls
        .iter()
        .filter(|u| {
            let t = crate::git_providers::js_trim(u).to_ascii_lowercase();
            t.starts_with("http://") || t.starts_with("https://")
        })
        .filter_map(|u| parse_remote(u).map(|r| r.host))
        .collect();
    let creds = credentials_for(db, data_dir, &hosts)?;
    if creds.is_empty() {
        return Ok(GitAuth::default());
    }
    let exe = std::env::current_exe().map_err(|e| HttpError::new(500, format!("Cannot locate the hubcli binary: {e}")))?;
    let json = serde_json::to_string(&creds).map_err(|e| HttpError::new(500, e.to_string()))?;
    Ok(GitAuth {
        args: vec![
            "-c".into(),
            "credential.helper=".into(),
            "-c".into(),
            format!("credential.helper=!{} {HELPER_COMMAND}", shq(&crate::paths::to_string(&exe))),
        ],
        env: vec![(CREDENTIALS_ENV.into(), json)],
    })
}

static MISSING_HTTPS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)could not read (?:Username|Password) for '(https?)://([^'/]+)").unwrap());
static FAILED_HTTPS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)Authentication failed for '(https?)://([^'/]+)").unwrap());
static HTTP_STATUS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)unable to access '(https?)://([^'/]+)[^']*': The requested URL returned error: (401|403)").unwrap());
static PUBLICKEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:[A-Za-z0-9_.-]+@)?([A-Za-z0-9_.-]+): Permission denied \(publickey").unwrap());
static HOSTKEY_FAILED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)Host key verification failed").unwrap());
static HOSTKEY_FOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)host key is known for ([A-Za-z0-9_.-]+)").unwrap());

/// Recognise "access refused" in git's output of a network operation.
pub fn auth_problem(output: &str, remote_hosts: &[String]) -> Option<GitAuthProblem> {
    let host_of = |s: &str| normalize_host(s).unwrap_or_else(|| s.to_string());
    let problem = |host: String, protocol, reason| Some(GitAuthProblem { host, protocol, reason });
    if let Some(m) = MISSING_HTTPS.captures(output) {
        return problem(host_of(&m[2]), GitAuthProtocol::Https, GitAuthReason::Missing);
    }
    if let Some(m) = FAILED_HTTPS.captures(output) {
        return problem(host_of(&m[2]), GitAuthProtocol::Https, GitAuthReason::Rejected);
    }
    if let Some(m) = HTTP_STATUS.captures(output) {
        return problem(host_of(&m[2]), GitAuthProtocol::Https, GitAuthReason::Rejected);
    }
    if let Some(m) = PUBLICKEY.captures(output) {
        return problem(host_of(&m[1]), GitAuthProtocol::Ssh, GitAuthReason::Missing);
    }
    if HOSTKEY_FAILED.is_match(output) {
        let host = match HOSTKEY_FOR.captures(output) {
            Some(m) => m[1].to_string(),
            None if remote_hosts.len() == 1 => remote_hosts[0].clone(),
            None => String::new(),
        };
        return problem(host, GitAuthProtocol::Ssh, GitAuthReason::Hostkey);
    }
    None
}

/// The git credential helper started by git for the hub's own git runs (see
/// `git_auth_for`). Answers `get` from `HUB_GIT_CREDENTIALS` (`{host: {username, password}}`),
/// which the hub puts only into the environment of that git process. `store` / `erase`
/// are ignored: logins are managed in the hub settings.
pub fn credential_helper(action: &str, input: &mut impl Read, output: &mut impl Write) -> std::io::Result<()> {
    if action != "get" {
        return Ok(());
    }
    let mut text = String::new();
    input.read_to_string(&mut text)?;
    let mut fields = BTreeMap::new();
    for line in text.split('\n') {
        if let Some(i) = line.find('=').filter(|&i| i > 0) {
            fields.insert(&line[..i], &line[i + 1..]);
        }
    }
    if !matches!(fields.get("protocol").copied(), Some("https" | "http")) {
        return Ok(());
    }
    let host = fields.get("host").copied().unwrap_or("").to_lowercase();
    let host = PORT.replace(&host, "");
    // malformed: answer nothing, git reports the access error itself
    let creds: BTreeMap<String, Login> =
        std::env::var(CREDENTIALS_ENV).ok().and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
    if let Some(c) = creds.get(host.as_ref()) {
        write!(output, "username={}\npassword={}\n", c.username, c.password)?;
    }
    Ok(())
}

// ---------- routes (Settings → Git access) ----------

use axum::extract::{Path as UrlPath, State};
use axum::routing::{get, put};
use axum::{Json, Router};

use crate::AppState;
use crate::http::JsonBody;

pub fn routes() -> Router<AppState> {
    Router::new().route("/git-credentials", get(list_route)).route("/git-credentials/{host}", put(set_route).delete(delete_route))
}

async fn list_route(State(s): State<AppState>) -> ApiResult<Json<Vec<GitCredentialInfo>>> {
    s.with(|db, _| list(db))
}

#[derive(serde::Deserialize)]
struct CredentialBody {
    username: String,
    kind: serde_json::Value,
    secret: String,
}

async fn set_route(
    State(s): State<AppState>,
    UrlPath(host): UrlPath<String>,
    JsonBody(b): JsonBody<CredentialBody>,
) -> ApiResult<Json<Vec<GitCredentialInfo>>> {
    let kind = match b.kind.as_str() {
        Some("token") => GitCredentialKind::Token,
        Some("password") => GitCredentialKind::Password,
        _ => return Err(HttpError::bad_request("kind: kind must be token or password")),
    };
    s.with(|db, cfg| set(db, &cfg.data_dir, &host, &b.username, kind, &b.secret))
}

async fn delete_route(State(s): State<AppState>, UrlPath(host): UrlPath<String>) -> ApiResult<Json<Vec<GitCredentialInfo>>> {
    s.with(|db, _| delete(db, &host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_hosts() {
        assert_eq!(normalize_host("https://user@GitLab.example.com:8443/x").as_deref(), Some("gitlab.example.com"));
        assert_eq!(normalize_host(" github.com ").as_deref(), Some("github.com"));
        assert_eq!(normalize_host("bad_host"), None);
        assert_eq!(normalize_host(""), None);
    }

    #[test]
    fn reads_a_git_credential_encrypted_by_the_typescript_server() {
        // produced by apps/server/src/git-auth.ts setGitCredential('github.com', {kind: 'token', secret: 'ghp_tökén'})
        // with HUBNESS_SECRET_KEY = the material below
        let keys = secrets::keys_from(&["test-key-material-that-is-32-chars-long!"]);
        let blob = "v1:08cd82e9:S/cVGSBkHljvqpRp:yzjHwGw+uxnbcaUDD8PDWg==:HjasIpX/B7At/YE=";
        assert_eq!(secrets::decrypt(&keys, NS, "github.com", blob).unwrap(), "ghp_tökén");
        assert!(secrets::decrypt(&keys, NS, "gitlab.com", blob).is_err(), "bound to the host");
    }

    #[test]
    fn recognises_access_refusals() {
        let p = auth_problem("fatal: could not read Username for 'https://GitHub.com': terminal prompts disabled", &[]).unwrap();
        assert_eq!((p.host.as_str(), p.protocol, p.reason), ("github.com", GitAuthProtocol::Https, GitAuthReason::Missing));
        let p = auth_problem("remote: x\nfatal: Authentication failed for 'https://gitlab.com/a/b.git/'", &[]).unwrap();
        assert_eq!(p.reason, GitAuthReason::Rejected);
        let p = auth_problem("fatal: unable to access 'https://h.io/a/': The requested URL returned error: 403", &[]).unwrap();
        assert_eq!((p.host.as_str(), p.reason), ("h.io", GitAuthReason::Rejected));
        let p = auth_problem("git@github.com: Permission denied (publickey).", &[]).unwrap();
        assert_eq!((p.host.as_str(), p.protocol), ("github.com", GitAuthProtocol::Ssh));
        let p = auth_problem("Host key verification failed.", &["only.example".into()]).unwrap();
        assert_eq!((p.host.as_str(), p.reason), ("only.example", GitAuthReason::Hostkey));
        assert!(auth_problem("fatal: not a git repository", &[]).is_none());
    }

    #[test]
    fn helper_answers_get_only() {
        let mut out = vec![];
        credential_helper("store", &mut "protocol=https\nhost=x\n".as_bytes(), &mut out).unwrap();
        assert!(out.is_empty());
        credential_helper("get", &mut "protocol=ssh\nhost=x\n".as_bytes(), &mut out).unwrap();
        assert!(out.is_empty());
    }
}
