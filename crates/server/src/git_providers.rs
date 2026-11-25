//! Git providers — a port of `git-providers.ts`: detecting the hosting from the remote URL,
//! and web links. New provider = new adapter in `PROVIDERS` (no changes to the rest of the code).

use std::sync::LazyLock;

use hubness_core::GitProviderId;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRemote {
    pub host: String,
    /// owner/repo (GitLab may have group/subgroup/repo)
    pub path: String,
}

static SCP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.-]+@([A-Za-z0-9_.-]+):(.+?)(?:\.git)?/?$").unwrap());
static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:ssh|https?|git)://(?:[^@/]+@)?([A-Za-z0-9_.-]+)(?::[0-9]+)?/(.+?)(?:\.git)?/?$").unwrap());
static CREDENTIALS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(https?://)[^@/]+@").unwrap());

/// JavaScript's `String.prototype.trim` whitespace (Rust's `trim` lacks U+FEFF).
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// `git@host:owner/repo.git`, `ssh://git@host[:port]/owner/repo.git`, `https://[user@]host/owner/repo(.git)` → host + path.
pub fn parse_remote(url: &str) -> Option<ParsedRemote> {
    let u = js_trim(url);
    // scp-like syntax; a path starting with "/" is not one (`(?!\/)` in the TS regex)
    if let Some(m) = SCP.captures(u).filter(|m| !m[2].starts_with('/')) {
        return Some(ParsedRemote { host: m[1].to_lowercase(), path: m[2].to_string() });
    }
    URL.captures(u).map(|m| ParsedRemote { host: m[1].to_lowercase(), path: m[2].to_string() })
}

/// URL without credentials (`https://user:token@host/...` → `https://host/...`).
pub fn strip_credentials(url: &str) -> String {
    CREDENTIALS.replace(url, "$1").into_owned()
}

/// Characters `encodeURIComponent` leaves as they are.
const URI_COMPONENT: &AsciiSet =
    &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'!').remove(b'~').remove(b'*').remove(b'\'').remove(b'(').remove(b')');

/// JavaScript's `encodeURIComponent`.
pub fn encode_uri_component(s: &str) -> String {
    utf8_percent_encode(s, URI_COMPONENT).to_string()
}

fn enc(s: &str) -> String {
    s.split('/').map(encode_uri_component).collect::<Vec<_>>().join("/")
}

pub struct Adapter {
    pub id: GitProviderId,
    pub label: &'static str,
    matches: fn(&ParsedRemote) -> bool,
    branch: fn(&str, &str) -> String,
    commit: fn(&str, &str) -> String,
    new_pull_request: fn(&str, &str, Option<&str>) -> String,
}

/// Web links of one repository.
pub struct Links<'a> {
    adapter: &'a Adapter,
    pub repo: String,
}

impl Links<'_> {
    pub fn branch(&self, b: &str) -> String {
        (self.adapter.branch)(&self.repo, b)
    }
    pub fn commit(&self, sha: &str) -> String {
        (self.adapter.commit)(&self.repo, sha)
    }
    pub fn new_pull_request(&self, head: &str, base: Option<&str>) -> String {
        (self.adapter.new_pull_request)(&self.repo, head, base)
    }
}

impl Adapter {
    pub fn links(&self, r: &ParsedRemote) -> Links<'_> {
        Links { adapter: self, repo: format!("https://{}/{}", r.host, r.path) }
    }
}

static PROVIDERS: &[Adapter] = &[
    Adapter {
        id: GitProviderId::Github,
        label: "GitHub",
        matches: |r| r.host == "github.com" || r.host.starts_with("github."),
        branch: |base, b| format!("{base}/tree/{}", enc(b)),
        commit: |base, sha| format!("{base}/commit/{sha}"),
        new_pull_request: |base, head, b| {
            format!("{base}/compare/{}{}?expand=1", b.map(|b| format!("{}...", enc(b))).unwrap_or_default(), enc(head))
        },
    },
    Adapter {
        id: GitProviderId::Gitlab,
        label: "GitLab",
        matches: |r| r.host == "gitlab.com" || r.host.starts_with("gitlab."),
        branch: |base, b| format!("{base}/-/tree/{}", enc(b)),
        commit: |base, sha| format!("{base}/-/commit/{sha}"),
        new_pull_request: |base, head, b| {
            format!(
                "{base}/-/merge_requests/new?merge_request%5Bsource_branch%5D={}{}",
                encode_uri_component(head),
                b.map(|b| format!("&merge_request%5Btarget_branch%5D={}", encode_uri_component(b))).unwrap_or_default()
            )
        },
    },
    Adapter {
        id: GitProviderId::Bitbucket,
        label: "Bitbucket",
        matches: |r| r.host == "bitbucket.org" || r.host.starts_with("bitbucket."),
        branch: |base, b| format!("{base}/src/{}", enc(b)),
        commit: |base, sha| format!("{base}/commits/{sha}"),
        new_pull_request: |base, head, b| {
            format!(
                "{base}/pull-requests/new?source={}{}",
                encode_uri_component(head),
                b.map(|b| format!("&dest={}", encode_uri_component(b))).unwrap_or_default()
            )
        },
    },
];

pub struct ResolvedProvider {
    pub id: GitProviderId,
    pub label: &'static str,
    pub adapter: Option<&'static Adapter>,
    pub remote: Option<ParsedRemote>,
}

impl ResolvedProvider {
    /// Web links, when the provider is known.
    pub fn links(&self) -> Option<Links<'static>> {
        Some(self.adapter?.links(self.remote.as_ref()?))
    }
}

/// Provider by remote URL; unknown host — generic (no web links).
pub fn resolve_provider(url: Option<&str>) -> Option<ResolvedProvider> {
    let url = url.filter(|u| !u.is_empty())?;
    let remote = parse_remote(url);
    let adapter = remote.as_ref().and_then(|r| PROVIDERS.iter().find(|p| (p.matches)(r)));
    Some(match adapter {
        Some(a) => ResolvedProvider { id: a.id, label: a.label, adapter: Some(a), remote },
        None => ResolvedProvider { id: GitProviderId::Generic, label: "Git", adapter: None, remote },
    })
}

/// Supported URL for git clone: https/ssh/git@host:path. Local paths and git options are not accepted.
pub fn is_clone_url(url: &str) -> bool {
    if url.chars().any(|c| c.is_whitespace() || c == '\u{feff}') || url.starts_with('-') {
        return false;
    }
    parse_remote(url).is_some()
}

/// Folder name from URL: `.../auth-service.git` → `auth-service`.
pub fn repo_name_from_url(url: &str) -> Option<String> {
    let r = parse_remote(url)?;
    let last = r.path.rsplit('/').next().filter(|l| !l.is_empty())?;
    Some(last.strip_suffix(".git").unwrap_or(last).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(url: &str) -> Option<(String, String)> {
        parse_remote(url).map(|r| (r.host, r.path))
    }

    #[test]
    fn parses_remotes_like_the_ts_server() {
        let gh = Some(("github.com".to_string(), "acme/app".to_string()));
        assert_eq!(parsed("git@GitHub.com:acme/app.git"), gh);
        assert_eq!(parsed("https://github.com/acme/app"), gh);
        assert_eq!(parsed("https://user:tok@github.com/acme/app.git/"), gh);
        assert_eq!(parsed("ssh://git@github.com:22/acme/app.git"), gh);
        assert_eq!(parsed("git@host:/abs/path"), None);
        assert_eq!(parsed("/local/path"), None);
        assert_eq!(parsed("https://gitlab.example.com/g/sub/repo.git").unwrap().1, "g/sub/repo");
    }

    #[test]
    fn providers_and_links() {
        let p = resolve_provider(Some("git@github.com:acme/app.git")).unwrap();
        assert_eq!((p.id, p.label), (GitProviderId::Github, "GitHub"));
        let l = p.links().unwrap();
        assert_eq!(l.repo, "https://github.com/acme/app");
        assert_eq!(l.branch("feat/x y"), "https://github.com/acme/app/tree/feat/x%20y");
        assert_eq!(l.new_pull_request("feat/x", Some("main")), "https://github.com/acme/app/compare/main...feat/x?expand=1");
        let gl = resolve_provider(Some("https://gitlab.com/a/b")).unwrap();
        assert_eq!(
            gl.links().unwrap().new_pull_request("f/x", Some("main")),
            "https://gitlab.com/a/b/-/merge_requests/new?merge_request%5Bsource_branch%5D=f%2Fx&merge_request%5Btarget_branch%5D=main"
        );
        let generic = resolve_provider(Some("https://example.com/a/b")).unwrap();
        assert_eq!((generic.id, generic.label), (GitProviderId::Generic, "Git"));
        assert!(generic.links().is_none());
        assert!(resolve_provider(None).is_none());
    }

    #[test]
    fn helpers() {
        assert_eq!(strip_credentials("https://u:t@host/x"), "https://host/x");
        assert_eq!(strip_credentials("git@host:x"), "git@host:x");
        assert!(is_clone_url("https://h/a/b") && !is_clone_url("-oProxy=x") && !is_clone_url("https://h/a b"));
        assert_eq!(repo_name_from_url("git@h:a/auth-service.git").as_deref(), Some("auth-service"));
        assert_eq!(encode_uri_component("a b/ü!*'()"), "a%20b%2F%C3%BC!*'()");
    }
}
