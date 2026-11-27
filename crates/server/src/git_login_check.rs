//! Is a stored git login still good? — a port of `git-login-check.ts`.
//!
//! The hub asks the host's API "who am I" with the login: the answer says whether it is
//! accepted, who it is there, and where the account's picture is — which the hub downloads
//! and keeps, so the browser shows it from the hub (a private host serves the picture only
//! with the login, and the browser asks no other server).
//!
//! Hosts are recognised by name (github.*, gitlab.*, bitbucket.org, codeberg.org); for any
//! other host the known APIs are tried in turn: GitLab, Gitea / Forgejo, GitHub Enterprise.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use hubness_core::{GitCredentialInfo, GitCredentialKind};
use reqwest::Url;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::AppState;
use crate::db::now;
use crate::error::{ApiResult, HttpError};
use crate::git_auth::{self, normalize_host};

const TIMEOUT: Duration = Duration::from_secs(8);
const MAX_AVATAR: usize = 512 * 1024;

pub struct Login {
    pub username: String,
    pub kind: GitCredentialKind,
    pub secret: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    Github,
    Gitlab,
    Gitea,
    Bitbucket,
}

/// One "who am I" request: where, and with which headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub api: Api,
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
}

fn basic(l: &Login) -> String {
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", l.username, l.secret)))
}

/// `HUBNESS_GIT_API` (tests) without trailing slashes.
fn hook() -> Option<String> {
    std::env::var("HUBNESS_GIT_API").ok().map(|b| b.trim_end_matches('/').to_string()).filter(|b| !b.is_empty())
}

/// Where a host's API lives. `HUBNESS_GIT_API` (tests) puts every host under one address:
/// `<base>/<api host>/…`.
fn api_root_with(base: Option<&str>, api_host: &str) -> String {
    match base {
        Some(base) => format!("{base}/{api_host}"),
        None => format!("https://{api_host}"),
    }
}

/// Which host's API an address belongs to (under the hook: the first part of the path).
fn api_host_of_with(base: Option<&str>, url: &Url) -> String {
    let href = url.as_str();
    if let Some(base) = base
        && (href == base || href.starts_with(&format!("{base}/")))
    {
        let rest = href.get(base.len() + 1..).unwrap_or("");
        return rest.split(['/', '?', '#']).next().unwrap_or("").to_string();
    }
    match (url.host_str(), url.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        _ => String::new(),
    }
}

/// The requests to try for a host, most likely first.
fn probes_with(base: Option<&str>, host: &str, l: &Login) -> Vec<Probe> {
    let root = |h: &str| api_root_with(base, h);
    let github = |root: String| Probe {
        api: Api::Github,
        url: format!("{root}/user"),
        headers: vec![("authorization", format!("Bearer {}", l.secret)), ("accept", "application/vnd.github+json".into())],
    };
    // a GitLab token works as a bearer; a password never does (the API takes tokens only)
    let gitlab = Probe {
        api: Api::Gitlab,
        url: format!("{}/api/v4/user", root(host)),
        headers: vec![("authorization", format!("Bearer {}", l.secret))],
    };
    // Gitea / Forgejo take a token, or the name and password
    let gitea = Probe {
        api: Api::Gitea,
        url: format!("{}/api/v1/user", root(host)),
        headers: vec![("authorization", if l.kind == GitCredentialKind::Token { format!("token {}", l.secret) } else { basic(l) })],
    };
    if host == "github.com" {
        return vec![github(root("api.github.com"))];
    }
    if host.starts_with("github.") {
        return vec![github(format!("{}/api/v3", root(host)))];
    }
    if host == "gitlab.com" || host.starts_with("gitlab.") {
        return vec![gitlab];
    }
    if host == "bitbucket.org" {
        return vec![Probe {
            api: Api::Bitbucket,
            url: format!("{}/2.0/user", root("api.bitbucket.org")),
            headers: vec![("authorization", basic(l))],
        }];
    }
    if host == "codeberg.org" || host.starts_with("gitea.") || host.starts_with("forgejo.") {
        return vec![gitea];
    }
    vec![gitlab, gitea, github(format!("{}/api/v3", root(host)))]
}

/// The account in a "who am I" answer: its name on the host and its picture.
pub fn account(api: Api, body: &Value) -> Option<(String, String)> {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let first = |keys: &[&str]| keys.iter().map(|k| s(&body[*k])).find(|v| !v.is_empty()).unwrap_or_default();
    if !body.is_object() {
        return None;
    }
    let name = match api {
        Api::Gitlab => first(&["username"]),
        Api::Bitbucket => first(&["username", "nickname", "display_name"]),
        Api::Github | Api::Gitea => first(&["login", "username"]),
    };
    if name.is_empty() {
        return None;
    }
    let avatar = if api == Api::Bitbucket { s(&body["links"]["avatar"]["href"]) } else { s(&body["avatar_url"]) };
    Some((name.chars().take(200).collect(), avatar))
}

/// A picture the hub keeps: an image type (no SVG — a document that can carry scripts, and it
/// would be served from the hub's own address) of a sane size.
pub fn avatar_ok(content_type: &str, len: usize) -> Option<String> {
    let t = content_type.split(';').next().unwrap_or("").trim().to_lowercase();
    let image = matches!(t.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif");
    (image && len > 0 && len <= MAX_AVATAR).then_some(t)
}

pub struct Avatar {
    pub content_type: String,
    pub data: Vec<u8>,
}

/// What a check found. `avatar`: `None` — keep the picture there is; `Some(None)` — there is none.
pub struct CheckResult {
    pub status: &'static str,
    pub error: String,
    pub account: String,
    pub avatar: Option<Option<Avatar>>,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder().timeout(TIMEOUT).user_agent("hubness").build().map_err(|e| e.to_string())
}

async fn get(client: &reqwest::Client, url: &str, headers: &[(&'static str, String)]) -> reqwest::Result<reqwest::Response> {
    let mut req = client.get(url);
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    req.send().await
}

/// The account's picture, else none. The login goes with the request only to the host that
/// just accepted it — never to a picture server (avatars.githubusercontent.com, gravatar).
async fn avatar(client: &reqwest::Client, base: Option<&str>, host: &str, probe: &Probe, raw: &str) -> Option<Avatar> {
    let probe_url = Url::parse(&probe.url).ok()?;
    // GitLab gives a path for uploaded pictures
    let url = Url::parse(&probe_url.origin().ascii_serialization()).ok()?.join(raw).ok()?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return None;
    }
    let owner = api_host_of_with(base, &url);
    let own = owner == api_host_of_with(base, &probe_url) || owner == host;
    let mut res = get(client, url.as_str(), if own { &probe.headers } else { &[] }).await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let content_type = res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    avatar_ok(&content_type, 1)?;
    // never more than the limit in memory, whatever the host sends
    let mut data = Vec::new();
    while let Some(chunk) = res.chunk().await.ok()? {
        data.extend_from_slice(&chunk);
        if data.len() > MAX_AVATAR {
            return None;
        }
    }
    let content_type = avatar_ok(&content_type, data.len())?;
    Some(Avatar { content_type, data })
}

/// Ask the host. Never fails: what went wrong is the result.
pub async fn check_login(host: &str, l: &Login) -> CheckResult {
    let base = hook();
    let base = base.as_deref();
    let none = |status, error: String| CheckResult { status, error, account: String::new(), avatar: None };
    let client = match client() {
        Ok(c) => c,
        Err(e) => return none("unknown", format!("Can’t reach {host}: {e}")),
    };
    let mut refused = 0u16;
    let mut unreachable = String::new();
    for probe in probes_with(base, host, l) {
        let res = match get(&client, &probe.url, &probe.headers).await {
            Ok(r) => r,
            Err(e) => {
                unreachable = if e.is_timeout() {
                    "timed out".into()
                } else if e.is_connect() {
                    "connection failed".into()
                } else {
                    e.to_string()
                };
                continue;
            }
        };
        let status = res.status().as_u16();
        if status == 401 || status == 403 {
            refused = status;
            continue;
        }
        if !res.status().is_success() {
            continue;
        }
        let Ok(body) = res.json::<Value>().await else { continue };
        let Some((name, avatar_url)) = account(probe.api, &body) else { continue };
        let picture = if avatar_url.is_empty() { None } else { avatar(&client, base, host, &probe, &avatar_url).await };
        return CheckResult { status: "ok", error: String::new(), account: name, avatar: Some(picture) };
    }
    // a refusal keeps the picture: it is still that account, it only has to sign in again
    if refused != 0 {
        // the APIs take tokens; a password that git itself accepts can't be checked this way
        if l.kind == GitCredentialKind::Password {
            return none(
                "unknown",
                format!("{host} doesn’t check passwords through its API: the login is checked by the first fetch or push."),
            );
        }
        return none("failed", format!("{host} refused the login ({refused}): the token is wrong, expired or revoked."));
    }
    if !unreachable.is_empty() {
        return none("unknown", format!("Can’t reach {host}: {unreachable}"));
    }
    none("unknown", format!("{host} has no API hubness knows: the login is checked by the first fetch or push."))
}

/// Remember a check. Who it is stays known when the host refuses or can't be asked: it is
/// still that account.
fn store(db: &Connection, host: &str, r: &CheckResult) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE git_credentials SET check_status = ?, checked_at = ?, check_error = ?, account = CASE WHEN ? != '' THEN ? ELSE account END WHERE host = ?",
        params![r.status, now(), r.error, r.account, r.account, host],
    )?;
    if let Some(avatar) = &r.avatar {
        let (data, kind) = match avatar {
            Some(a) => (Some(a.data.as_slice()), a.content_type.as_str()),
            None => (None, ""),
        };
        db.execute("UPDATE git_credentials SET avatar = ?, avatar_type = ? WHERE host = ?", params![data, kind, host])?;
    }
    Ok(())
}

/// `POST /git-credentials/:host/check`: check the stored login and remember the answer.
pub async fn check_route(State(s): State<AppState>, UrlPath(host): UrlPath<String>) -> ApiResult<Json<Vec<GitCredentialInfo>>> {
    let missing = || HttpError::not_found("No login for this host");
    let host = normalize_host(&host).ok_or_else(missing)?;
    // the database is not held while the host is asked
    let (login, updated_at) = {
        let db = s.db()?;
        let row = git_auth::rows(&db)?.into_iter().find(|r| r.host == host).ok_or_else(missing)?;
        let secret = git_auth::secret_of(&s.config().data_dir, &row)?
            .ok_or_else(|| HttpError::new(409, "The stored login can’t be decrypted (the secret key changed): save it again"))?;
        (Login { username: row.username, kind: git_auth::kind_of(&row.kind), secret }, row.updated_at)
    };
    let result = check_login(&host, &login).await;
    let db = s.db()?;
    // the login may have been replaced or removed while the host was thinking
    if git_auth::rows(&db)?.iter().any(|r| r.host == host && r.updated_at == updated_at) {
        store(&db, &host, &result)?;
    }
    Ok(Json(git_auth::list(&db)?))
}

/// `GET /git-credentials/:host/avatar`: an image and nothing else, whatever the host sent.
pub async fn avatar_route(State(s): State<AppState>, UrlPath(host): UrlPath<String>) -> ApiResult<Response> {
    let missing = || HttpError::not_found("No picture");
    let host = normalize_host(&host).ok_or_else(missing)?;
    let found: Option<(Option<Vec<u8>>, String)> = {
        let db = s.db()?;
        db.query_row("SELECT avatar, avatar_type FROM git_credentials WHERE host = ?", [&host], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
    };
    let Some((Some(data), kind)) = found else { return Err(missing()) };
    let content_type = HeaderValue::from_str(&kind).map_err(|_| missing())?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=86400")),
            (header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
            (header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox")),
        ],
        data,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn login(kind: GitCredentialKind) -> Login {
        Login { username: "ann".into(), kind, secret: "s3".into() }
    }

    fn urls(base: Option<&str>, host: &str) -> Vec<(Api, String)> {
        probes_with(base, host, &login(GitCredentialKind::Token)).into_iter().map(|p| (p.api, p.url)).collect()
    }

    #[test]
    fn each_host_gets_the_requests_of_its_api() {
        assert_eq!(urls(None, "github.com"), [(Api::Github, "https://api.github.com/user".to_string())]);
        assert_eq!(urls(None, "github.corp.example"), [(Api::Github, "https://github.corp.example/api/v3/user".to_string())]);
        assert_eq!(urls(None, "gitlab.com"), [(Api::Gitlab, "https://gitlab.com/api/v4/user".to_string())]);
        assert_eq!(urls(None, "gitlab.gostex.io"), [(Api::Gitlab, "https://gitlab.gostex.io/api/v4/user".to_string())]);
        assert_eq!(urls(None, "bitbucket.org"), [(Api::Bitbucket, "https://api.bitbucket.org/2.0/user".to_string())]);
        assert_eq!(urls(None, "codeberg.org"), [(Api::Gitea, "https://codeberg.org/api/v1/user".to_string())]);
        assert_eq!(urls(None, "forgejo.example"), [(Api::Gitea, "https://forgejo.example/api/v1/user".to_string())]);
        // a host by another name: every API the hub knows, GitLab first
        assert_eq!(
            urls(None, "git.acme.example"),
            [
                (Api::Gitlab, "https://git.acme.example/api/v4/user".to_string()),
                (Api::Gitea, "https://git.acme.example/api/v1/user".to_string()),
                (Api::Github, "https://git.acme.example/api/v3/user".to_string()),
            ]
        );
        // the test hook puts every host under one address
        assert_eq!(urls(Some("http://127.0.0.1:9"), "github.com"), [(Api::Github, "http://127.0.0.1:9/api.github.com/user".to_string())]);
        assert_eq!(urls(Some("http://127.0.0.1:9"), "bitbucket.org")[0].1, "http://127.0.0.1:9/api.bitbucket.org/2.0/user");
    }

    #[test]
    fn the_login_is_sent_the_way_each_api_takes_it() {
        let auth = |host: &str, kind| probes_with(None, host, &login(kind)).into_iter().map(|p| p.headers[0].1.clone()).collect::<Vec<_>>();
        let basic = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("ann:s3"));
        assert_eq!(auth("github.com", GitCredentialKind::Token), ["Bearer s3"]);
        assert_eq!(auth("gitlab.com", GitCredentialKind::Password), ["Bearer s3"], "GitLab's API takes tokens only");
        assert_eq!(auth("codeberg.org", GitCredentialKind::Token), ["token s3"]);
        assert_eq!(auth("codeberg.org", GitCredentialKind::Password), std::slice::from_ref(&basic));
        assert_eq!(auth("bitbucket.org", GitCredentialKind::Password), std::slice::from_ref(&basic));
        assert_eq!(auth("git.acme.example", GitCredentialKind::Password), ["Bearer s3".to_string(), basic, "Bearer s3".to_string()]);
    }

    #[test]
    fn the_account_is_read_from_each_apis_answer() {
        let gh = json!({ "login": "octocat", "name": "The Octocat", "avatar_url": "https://a/1.png" });
        assert_eq!(account(Api::Github, &gh), Some(("octocat".into(), "https://a/1.png".into())));
        assert_eq!(
            account(Api::Gitlab, &json!({ "username": "r.denis", "avatar_url": "/uploads/a.png" })),
            Some(("r.denis".into(), "/uploads/a.png".into()))
        );
        assert_eq!(account(Api::Gitlab, &gh), None, "GitLab names the account `username`");
        assert_eq!(account(Api::Gitea, &json!({ "username": "ann" })), Some(("ann".into(), String::new())));
        let bb = json!({ "display_name": "B B", "nickname": "bb", "links": { "avatar": { "href": "https://b/a" } } });
        assert_eq!(account(Api::Bitbucket, &bb), Some(("bb".into(), "https://b/a".into())));
        for junk in [json!(null), json!([]), json!("x"), json!({}), json!({ "login": 5 }), json!({ "login": "" })] {
            assert_eq!(account(Api::Github, &junk), None, "{junk}");
        }
        assert_eq!(account(Api::Github, &json!({ "login": "é".repeat(300) })).unwrap().0.chars().count(), 200);
    }

    #[test]
    fn an_address_belongs_to_a_host_also_under_the_hook() {
        let of = |base: Option<&str>, u: &str| api_host_of_with(base, &Url::parse(u).unwrap());
        assert_eq!(of(None, "https://api.github.com/user"), "api.github.com");
        assert_eq!(of(None, "https://git.example:8443/x"), "git.example:8443");
        let base = Some("http://127.0.0.1:9");
        assert_eq!(of(base, "http://127.0.0.1:9/api.github.com/user"), "api.github.com");
        assert_eq!(of(base, "http://127.0.0.1:9/avatars.example/u/1.png?s=40"), "avatars.example");
        assert_eq!(of(base, "http://127.0.0.1:9/gitlab.corp.example"), "gitlab.corp.example");
        assert_eq!(of(base, "https://elsewhere.example/a.png"), "elsewhere.example", "not under the hook: its own host");
        assert_ne!(
            of(base, "http://127.0.0.1:9/avatars.example/u/1.png"),
            of(base, "http://127.0.0.1:9/api.github.com/user"),
            "a picture server is not the API host"
        );
    }

    #[test]
    fn only_a_real_picture_of_a_sane_size_is_kept() {
        assert_eq!(avatar_ok("image/png", 100).as_deref(), Some("image/png"));
        assert_eq!(avatar_ok("Image/JPEG; charset=binary", 100).as_deref(), Some("image/jpeg"));
        for t in ["image/gif", "image/webp", "image/avif"] {
            assert!(avatar_ok(t, 1).is_some(), "{t}");
        }
        assert_eq!(avatar_ok("image/svg+xml", 100), None, "an SVG is a document: not served from the hub");
        assert_eq!(avatar_ok("text/html", 100), None);
        assert_eq!(avatar_ok("", 100), None);
        assert_eq!(avatar_ok("image/png", 0), None);
        assert!(avatar_ok("image/png", MAX_AVATAR).is_some());
        assert_eq!(avatar_ok("image/png", MAX_AVATAR + 1), None);
    }
}
