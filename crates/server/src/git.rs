//! Git panel of a project — a port of `git.ts` and `routes/git.ts`: status, diff, branches,
//! log, stashes and operations. Every call runs the `git` CLI with prompts, optional locks
//! and editors disabled and a C locale (the output is parsed).

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;

use hubness_core::{GitBranch, GitCommit, GitFile, GitLogResponse, GitOpResult, GitRepoState, GitStash, GitStatus};
use regex::Regex;

use crate::AppState;
use crate::git_auth::{self, GitAuth};
use crate::git_providers::parse_remote;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const LONG_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Exec {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run git in `cwd`. Like Node's `execFile`: a failure to start, a signal or a timeout is
/// exit code 1, and a failed run without stderr reports `Command failed: git …`.
pub async fn run(cwd: &Path, args: &[&str], timeout: Duration, extra_env: &[(String, String)]) -> Exec {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(crate::config::child_env())
        // GIT_EDITOR=true: --continue and merges keep the prepared message instead of waiting for an editor
        .envs([("GIT_TERMINAL_PROMPT", "0"), ("GIT_OPTIONAL_LOCKS", "0"), ("GIT_EDITOR", "true"), ("LC_ALL", "C")])
        .envs(extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let failed = |stderr: String| Exec { code: 1, stdout: String::new(), stderr };
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return failed(format!("spawn git: {e}")),
    };
    let out = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return failed(e.to_string()),
        Err(_) => return failed(format!("Command failed: git {} (timed out)", args.join(" "))),
    };
    let code = out.status.code().unwrap_or(1);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if code != 0 && stderr.is_empty() {
        stderr = format!("Command failed: git {}\n", args.join(" "));
    }
    Exec { code, stdout, stderr }
}

async fn git(cwd: &Path, args: &[&str]) -> Exec {
    run(cwd, args, DEFAULT_TIMEOUT, &[]).await
}

fn op(r: Exec) -> GitOpResult {
    let sep = if !r.stderr.is_empty() && !r.stdout.is_empty() { "\n" } else { "" };
    GitOpResult { ok: r.code == 0, output: format!("{}{sep}{}", r.stdout, r.stderr).trim().to_string(), auth: None }
}

/// Fetch and push URLs of all remotes.
async fn remote_urls(cwd: &Path) -> Vec<String> {
    let r = git(cwd, &["config", "--get-regexp", r"^remote\..*\.(url|pushurl)$"]).await;
    r.stdout.split('\n').map(|l| l.find(' ').map(|i| &l[i + 1..]).unwrap_or(l).trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// Fetch / pull / push: with the stored logins of the remotes' hosts; an access refusal is recognised.
async fn network_op(s: &AppState, cwd: &Path, args: &[&str]) -> crate::error::ApiResult<GitOpResult> {
    let urls = remote_urls(cwd).await;
    let auth: GitAuth = {
        let db = s.db()?;
        git_auth::git_auth_for(&db, &s.config.data_dir, &urls)?
    };
    let full: Vec<&str> = auth.args.iter().map(String::as_str).chain(args.iter().copied()).collect();
    let res = op(run(cwd, &full, LONG_TIMEOUT, &auth.env).await);
    if res.ok {
        return Ok(res);
    }
    let mut hosts: Vec<String> = vec![];
    for h in urls.iter().filter_map(|u| parse_remote(u).map(|r| r.host)) {
        if !h.is_empty() && !hosts.contains(&h) {
            hosts.push(h);
        }
    }
    let auth = git_auth::auth_problem(&res.output, &hosts);
    Ok(GitOpResult { auth, ..res })
}

pub async fn is_repo(cwd: &Path) -> bool {
    let r = git(cwd, &["rev-parse", "--is-inside-work-tree"]).await;
    r.code == 0 && r.stdout.trim() == "true"
}

static AHEAD_BEHIND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\+([0-9]+) -([0-9]+)").unwrap());

pub async fn status(cwd: &Path) -> GitStatus {
    let mut st = GitStatus {
        is_repo: false,
        branch: None,
        upstream: None,
        ahead: 0,
        behind: 0,
        detached: false,
        files: vec![],
        state: None,
        stashes: 0,
    };
    let r = git(cwd, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"]).await;
    if r.code != 0 {
        return st;
    }
    let (git_dir, stash) = tokio::join!(git(cwd, &["rev-parse", "--absolute-git-dir"]), git(cwd, &["stash", "list", "--format=%gd"]));
    st.is_repo = true;
    st.state = repo_state(git_dir.stdout.trim());
    st.stashes = stash.stdout.split('\n').filter(|l| !l.is_empty()).count();
    let parts: Vec<&str> = r.stdout.split('\0').collect();
    let mut i = 0;
    while i < parts.len() {
        let line = parts[i];
        // the n-th space-separated field; the last one (a path) may contain spaces
        let field = |n: usize, count: usize| line.splitn(count, ' ').nth(n).unwrap_or("").to_string();
        if let Some(h) = line.strip_prefix("# branch.head ") {
            st.detached = h == "(detached)";
            st.branch = (!st.detached).then(|| h.to_string());
        } else if let Some(u) = line.strip_prefix("# branch.upstream ") {
            st.upstream = Some(u.to_string());
        } else if line.starts_with("# branch.ab ") {
            if let Some(m) = AHEAD_BEHIND.captures(line) {
                st.ahead = m[1].parse().unwrap_or(0);
                st.behind = m[2].parse().unwrap_or(0);
            }
        } else if line.starts_with("1 ") {
            st.files.push(file(&field(1, 9), field(8, 9)));
        } else if line.starts_with("2 ") {
            let mut g = file(&field(1, 10), field(9, 10));
            i += 1;
            g.from = parts.get(i).map(|s| s.to_string());
            st.files.push(g);
        } else if line.starts_with("u ") {
            st.files.push(GitFile { conflicted: true, ..file(&field(1, 11), field(10, 11)) });
        } else if let Some(p) = line.strip_prefix("? ") {
            st.files.push(GitFile {
                path: p.to_string(),
                index: " ".into(),
                worktree: "?".into(),
                untracked: true,
                conflicted: false,
                from: None,
            });
        }
        i += 1;
    }
    st
}

/// Half-done operation, from the marker files git leaves in .git.
fn repo_state(git_dir: &str) -> Option<GitRepoState> {
    if git_dir.is_empty() {
        return None;
    }
    let has = |f: &str| Path::new(git_dir).join(f).exists();
    if has("rebase-merge") || has("rebase-apply") {
        Some(GitRepoState::Rebase)
    } else if has("MERGE_HEAD") {
        Some(GitRepoState::Merge)
    } else if has("CHERRY_PICK_HEAD") {
        Some(GitRepoState::CherryPick)
    } else if has("REVERT_HEAD") {
        Some(GitRepoState::Revert)
    } else {
        None
    }
}

fn file(xy: &str, path: String) -> GitFile {
    let norm = |c: Option<char>| match c {
        Some('.') | None => " ".to_string(),
        Some(c) => c.to_string(),
    };
    let mut chars = xy.chars();
    GitFile { path, index: norm(chars.next()), worktree: norm(chars.next()), untracked: false, conflicted: false, from: None }
}

/// Unified diff of a single file or of all changes.
pub async fn diff(cwd: &Path, path: Option<&str>, staged: bool, untracked: bool) -> String {
    if let (Some(path), true) = (path, untracked) {
        // exit code 1 means "there are differences"
        return git(cwd, &["diff", "--no-index", "--no-color", "--", "/dev/null", path]).await.stdout;
    }
    let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
    if staged {
        args.push("--cached");
    }
    if let Some(p) = path {
        args.extend(["--", p]);
    }
    git(cwd, &args).await.stdout
}

pub async fn branches(cwd: &Path) -> Vec<GitBranch> {
    let r = git(
        cwd,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)%00%(refname:short)%00%(HEAD)%00%(upstream:short)%00%(committerdate:relative)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .await;
    if r.code != 0 {
        return vec![];
    }
    r.stdout
        .split('\n')
        .filter(|l| !l.is_empty())
        .map(|l| l.split('\0').collect::<Vec<_>>())
        .filter(|f| !f[0].ends_with("/HEAD"))
        .map(|f| {
            let at = |i: usize| f.get(i).copied().unwrap_or("");
            GitBranch {
                name: at(1).to_string(),
                current: at(2) == "*",
                remote: at(0).starts_with("refs/remotes/"),
                upstream: Some(at(3).to_string()).filter(|u| !u.is_empty()),
                last_commit: at(4).to_string(),
            }
        })
        .collect()
}

/// JavaScript's `String(n)` for the numbers `log` can get (integers, or a stray fraction).
fn js_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e21 { format!("{}", n as i64) } else { format!("{n}") }
}

/// `limit` is what `Math.min(2000, Number(query) || 150)` gives.
pub async fn log(cwd: &Path, limit: f64, all: bool) -> GitLogResponse {
    const S: &str = "\x1f";
    let format = format!("--format=%H{S}%h{S}%P{S}%s{S}%an{S}%ad{S}%D");
    let n = js_num(limit + 1.0);
    let mut args = vec!["log"];
    if all {
        args.push("--all");
    }
    args.extend(["--topo-order", "--date=relative", &format, "-n", &n]);
    let r = git(cwd, &args).await;
    if r.code != 0 {
        return GitLogResponse { commits: vec![], more: false };
    }
    let mut commits: Vec<GitCommit> = r
        .stdout
        .split('\n')
        .filter(|l| !l.is_empty())
        .map(|line| {
            let f: Vec<&str> = line.split(S).collect();
            let at = |i: usize| f.get(i).copied().unwrap_or("").to_string();
            let refs = at(6);
            GitCommit {
                hash: at(0),
                short: at(1),
                parents: at(2).split(' ').filter(|p| !p.is_empty()).map(String::from).collect(),
                subject: at(3),
                author: at(4),
                date: at(5),
                refs: if refs.is_empty() { vec![] } else { refs.split(", ").map(String::from).collect() },
            }
        })
        .collect();
    let len = commits.len();
    let more = len as f64 > limit;
    // Array.prototype.slice(0, limit)
    let end = limit.trunc();
    let end = if end < 0.0 { (len as f64 + end).max(0.0) as usize } else { (end as usize).min(len) };
    commits.truncate(end);
    GitLogResponse { commits, more }
}

/// Patch of one commit (first parent for merges) or one stash entry.
pub async fn show(cwd: &Path, rev: &str) -> String {
    if !valid_rev(rev) {
        return String::new();
    }
    let r = if rev.starts_with("stash@") {
        git(cwd, &["stash", "show", "-p", "--include-untracked", "--no-color", rev]).await
    } else {
        git(cwd, &["show", "--format=", "--no-color", "--diff-merges=first-parent", rev]).await
    };
    r.stdout
}

static STASH_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^On [^:]+: ").unwrap());

pub async fn stashes(cwd: &Path) -> Vec<GitStash> {
    const S: &str = "\x1f";
    let r = git(cwd, &["stash", "list", &format!("--format=%gd{S}%gs{S}%cr")]).await;
    if r.code != 0 {
        return vec![];
    }
    r.stdout
        .split('\n')
        .filter(|l| !l.is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split(S).collect();
            let at = |i: usize| f.get(i).copied().unwrap_or("");
            GitStash { reference: at(0).to_string(), message: STASH_PREFIX.replace(at(1), "").into_owned(), date: at(2).to_string() }
        })
        .collect()
}

/// Branch / commit / stash names from the UI: no options, no shell-ish characters.
pub fn valid_rev(rev: &str) -> bool {
    let ok_char = |c: char| c.is_ascii_alphanumeric() || "_./@{}^~-".contains(c);
    (1..=200).contains(&rev.len()) && rev.chars().all(ok_char) && !rev.starts_with('-') && !rev.contains("..")
}

fn bad(what: &str) -> GitOpResult {
    GitOpResult::fail(format!("Invalid {what}"))
}

static REMOTE_BRANCH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([^/]+)/(.+)$").unwrap());
static STASH_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^stash@\{[0-9]+\}$").unwrap());

/// The operations of `POST /projects/:id/git/:op` (`ops` in git.ts).
pub mod ops {
    use super::*;
    use crate::error::ApiResult;

    pub async fn fetch(s: &AppState, cwd: &Path) -> ApiResult<GitOpResult> {
        network_op(s, cwd, &["fetch", "--all", "--prune"]).await
    }

    pub async fn pull(s: &AppState, cwd: &Path) -> ApiResult<GitOpResult> {
        network_op(s, cwd, &["pull", "--ff-only"]).await
    }

    pub async fn push(s: &AppState, cwd: &Path) -> ApiResult<GitOpResult> {
        let st = status(cwd).await;
        let Some(branch) = st.branch else { return Ok(GitOpResult::fail("Detached HEAD — nothing to push")) };
        if st.upstream.is_some() {
            return network_op(s, cwd, &["push"]).await;
        }
        network_op(s, cwd, &["push", "-u", "origin", &branch]).await
    }

    pub async fn commit(cwd: &Path, message: &str, all: bool) -> GitOpResult {
        if crate::git_providers::js_trim(message).is_empty() {
            return GitOpResult::fail("Empty commit message");
        }
        if all {
            let a = git(cwd, &["add", "-A"]).await;
            if a.code != 0 {
                return op(a);
            }
        }
        op(git(cwd, &["commit", "-m", message]).await)
    }

    fn with_paths<'a>(head: &[&'a str], paths: &'a [String]) -> Vec<&'a str> {
        head.iter().copied().chain(paths.iter().map(String::as_str)).collect()
    }

    pub async fn stage(cwd: &Path, paths: &[String]) -> GitOpResult {
        op(git(cwd, &with_paths(&["add", "--"], paths)).await)
    }

    pub async fn unstage(cwd: &Path, paths: &[String]) -> GitOpResult {
        let r = git(cwd, &with_paths(&["restore", "--staged", "--"], paths)).await;
        if r.code == 0 {
            return op(r);
        }
        // repository without commits — no HEAD yet
        op(git(cwd, &with_paths(&["rm", "--cached", "-r", "--quiet", "--"], paths)).await)
    }

    pub async fn checkout(cwd: &Path, branch: &str, create: bool) -> GitOpResult {
        if create {
            return op(git(cwd, &["switch", "-c", branch]).await);
        }
        if let Some(m) = REMOTE_BRANCH.captures(branch) {
            let local = git(cwd, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{}", &m[2])]).await;
            let is_remote = git(cwd, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{branch}")]).await;
            if local.code != 0 && is_remote.code == 0 {
                return op(git(cwd, &["switch", "--track", branch]).await);
            }
        }
        op(git(cwd, &["switch", branch]).await)
    }

    pub async fn branch_at(cwd: &Path, name: &str, rev: &str) -> GitOpResult {
        if !valid_rev(name) || !valid_rev(rev) {
            return bad("branch or commit");
        }
        op(git(cwd, &["switch", "-c", name, rev]).await)
    }

    pub async fn delete_branch(cwd: &Path, branch: &str, force: bool) -> GitOpResult {
        if !valid_rev(branch) {
            return bad("branch");
        }
        op(git(cwd, &["branch", if force { "-D" } else { "-d" }, branch]).await)
    }

    pub async fn merge(cwd: &Path, branch: &str, no_ff: bool) -> GitOpResult {
        if !valid_rev(branch) {
            return bad("branch");
        }
        let mut args = vec!["merge", "--no-edit"];
        if no_ff {
            args.push("--no-ff");
        }
        args.push(branch);
        op(run(cwd, &args, LONG_TIMEOUT, &[]).await)
    }

    pub async fn rebase(cwd: &Path, onto: &str) -> GitOpResult {
        if !valid_rev(onto) {
            return bad("branch");
        }
        op(run(cwd, &["rebase", onto], LONG_TIMEOUT, &[]).await)
    }

    pub async fn reset(cwd: &Path, rev: &str, mode: &str) -> GitOpResult {
        if !valid_rev(rev) {
            return bad("commit");
        }
        if !["soft", "mixed", "hard"].contains(&mode) {
            return bad("mode");
        }
        op(git(cwd, &["reset", &format!("--{mode}"), rev]).await)
    }

    pub async fn cherry_pick(cwd: &Path, rev: &str) -> GitOpResult {
        if !valid_rev(rev) {
            return bad("commit");
        }
        op(git(cwd, &["cherry-pick", rev]).await)
    }

    pub async fn revert(cwd: &Path, rev: &str) -> GitOpResult {
        if !valid_rev(rev) {
            return bad("commit");
        }
        op(git(cwd, &["revert", "--no-edit", rev]).await)
    }

    /// Continue or abort whatever operation is half-done.
    pub async fn proceed(cwd: &Path, action: &str) -> GitOpResult {
        let st = status(cwd).await;
        let Some(state) = st.state else { return GitOpResult::fail("Nothing in progress") };
        if action == "continue" && st.files.iter().any(|f| f.conflicted) {
            return GitOpResult::fail("Resolve and stage all conflicted files first");
        }
        op(run(cwd, &[state.command(), &format!("--{action}")], LONG_TIMEOUT, &[]).await)
    }

    /// Throw away local changes of files: tracked ones back to HEAD, untracked ones deleted.
    pub async fn discard(cwd: &Path, paths: &[String]) -> GitOpResult {
        let st = status(cwd).await;
        let untracked: Vec<String> = st.files.iter().filter(|f| f.untracked && paths.contains(&f.path)).map(|f| f.path.clone()).collect();
        let tracked: Vec<String> = paths.iter().filter(|p| !untracked.contains(p)).cloned().collect();
        if !tracked.is_empty() {
            let r = git(cwd, &with_paths(&["restore", "--source=HEAD", "--staged", "--worktree", "--"], &tracked)).await;
            if r.code != 0 {
                return op(r);
            }
        }
        if !untracked.is_empty() {
            return op(git(cwd, &with_paths(&["clean", "-f", "-q", "--"], &untracked)).await);
        }
        GitOpResult { ok: true, output: String::new(), auth: None }
    }

    pub async fn stash_push(cwd: &Path, message: &str, untracked: bool) -> GitOpResult {
        let message = crate::git_providers::js_trim(message);
        let mut args = vec!["stash", "push"];
        if untracked {
            args.push("--include-untracked");
        }
        if !message.is_empty() {
            args.extend(["-m", message]);
        }
        op(git(cwd, &args).await)
    }

    pub async fn stash(cwd: &Path, action: &str, reference: &str) -> GitOpResult {
        if !STASH_REF.is_match(reference) {
            return bad("stash");
        }
        op(git(cwd, &["stash", action, reference]).await)
    }
}

/// Repository info: remote origin, current branch, default branch (origin/HEAD, else main/master).
pub struct RepoBasics {
    pub is_repo: bool,
    pub remote_url: Option<String>,
    pub branch: Option<String>,
    pub default_branch: Option<String>,
}

pub async fn repo_basics(cwd: &Path) -> RepoBasics {
    if !is_repo(cwd).await {
        return RepoBasics { is_repo: false, remote_url: None, branch: None, default_branch: None };
    }
    let (remote, branch, head) = tokio::join!(
        git(cwd, &["remote", "get-url", "origin"]),
        git(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"]),
        git(cwd, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]),
    );
    let trimmed = |r: &Exec| Some(r.stdout.trim().to_string()).filter(|s| r.code == 0 && !s.is_empty());
    let mut default_branch = trimmed(&head).map(|h| h.strip_prefix("origin/").map(String::from).unwrap_or(h)).filter(|h| !h.is_empty());
    if default_branch.is_none() {
        for b in ["main", "master"] {
            if git(cwd, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")]).await.code == 0 {
                default_branch = Some(b.to_string());
                break;
            }
        }
    }
    RepoBasics { is_repo: true, remote_url: trimmed(&remote), branch: trimmed(&branch), default_branch }
}

// ---------- routes ----------

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Map, Value, json};

use crate::error::{ApiResult, HttpError};

pub fn routes() -> Router<AppState> {
    Router::new().route("/projects/{id}/git/{op}", get(read_route).post(op_route))
}

/// Project repository folder (a project = exactly one repository).
fn root_path(s: &AppState, id: &str) -> ApiResult<std::path::PathBuf> {
    Ok(s.with(|db, _| crate::catalog::project_or_404(db, id))?.0.path.into())
}

/// `Number(v)` of JavaScript for a query value (`undefined` → NaN).
fn js_number_of(v: Option<&str>) -> f64 {
    let Some(v) = v else { return f64::NAN };
    let t = crate::git_providers::js_trim(v);
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ if t.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c)) => t.parse().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

/// GET routes of the git panel (one path pattern shares `POST /git/:op`).
async fn read_route(
    State(s): State<AppState>,
    UrlPath((id, what)): UrlPath<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    let qs = |k: &str| q.get(k).map(String::as_str);
    if !["status", "diff", "branches", "log", "show", "stashes"].contains(&what.as_str()) {
        return Err(HttpError::not_found("Not found"));
    }
    let cwd = root_path(&s, &id)?;
    Ok(match what.as_str() {
        "status" => Json(status(&cwd).await).into_response(),
        "diff" => {
            let path = qs("path").filter(|p| !p.is_empty());
            Json(json!({ "diff": diff(&cwd, path, qs("staged") == Some("1"), qs("untracked") == Some("1")).await })).into_response()
        }
        "branches" => Json(branches(&cwd).await).into_response(),
        "log" => {
            let n = js_number_of(Some(qs("limit").unwrap_or("150")));
            let limit = if n.is_nan() || n == 0.0 { 150.0 } else { n.min(2000.0) };
            Json(log(&cwd, limit, qs("all") != Some("0")).await).into_response()
        }
        "show" => Json(json!({ "diff": show(&cwd, qs("rev").unwrap_or("")).await })).into_response(),
        _ => Json(stashes(&cwd).await).into_response(),
    })
}

/// One body for every operation: each op reads the fields it needs (no body = defaults).
#[derive(Default)]
struct GitOp {
    message: Option<String>,
    all: Option<bool>,
    paths: Option<Vec<String>>,
    branch: Option<String>,
    create: Option<bool>,
    rev: Option<String>,
    mode: Option<String>,
    force: Option<bool>,
    no_ff: Option<bool>,
    untracked: Option<bool>,
    action: Option<String>,
}

fn js_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn expected(field: &str, want: &str, got: &Value) -> HttpError {
    HttpError::bad_request(format!("{field}: Invalid input: expected {want}, received {}", js_type(got)))
}

/// Check the body like the zod schema of routes/git.ts (field order = the first issue reported).
fn parse_op_body(raw: Value) -> ApiResult<GitOp> {
    let Value::Object(obj) = raw else {
        return Err(HttpError::bad_request(format!("Invalid input: expected object, received {}", js_type(&raw))));
    };
    let get = |k: &str| obj.get(k);
    let string = |k: &str, max: Option<usize>| -> ApiResult<Option<String>> {
        match get(k) {
            None => Ok(None),
            Some(Value::String(s)) => match max {
                Some(m) if s.encode_utf16().count() > m => {
                    Err(HttpError::bad_request(format!("{k}: Too big: expected string to have <={m} characters")))
                }
                _ => Ok(Some(s.clone())),
            },
            Some(v) => Err(expected(k, "string", v)),
        }
    };
    let boolean = |k: &str| -> ApiResult<Option<bool>> {
        match get(k) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(v) => Err(expected(k, "boolean", v)),
        }
    };
    let message = string("message", Some(100_000))?;
    let all = boolean("all")?;
    let paths = match get("paths") {
        None => None,
        Some(Value::Array(a)) => {
            let mut out = vec![];
            for (i, v) in a.iter().enumerate() {
                match v {
                    Value::String(s) => out.push(s.clone()),
                    v => return Err(expected(&format!("paths.{i}"), "string", v)),
                }
            }
            Some(out)
        }
        Some(v) => return Err(expected("paths", "array", v)),
    };
    let branch = string("branch", Some(500))?;
    let create = boolean("create")?;
    let rev = string("rev", Some(500))?;
    let mode = match get("mode") {
        None => None,
        Some(Value::String(m)) if ["soft", "mixed", "hard"].contains(&m.as_str()) => Some(m.clone()),
        Some(_) => return Err(HttpError::bad_request(r#"mode: Invalid option: expected one of "soft"|"mixed"|"hard""#)),
    };
    let force = boolean("force")?;
    let no_ff = boolean("noFf")?;
    let untracked = boolean("untracked")?;
    let action = string("action", None)?;
    Ok(GitOp { message, all, paths, branch, create, rev, mode, force, no_ff, untracked, action })
}

/// Lenient body: no body / not JSON / broken JSON counts as `{}`.
fn lenient_body(headers: &HeaderMap, body: &Bytes) -> Value {
    let is_json = headers.get(axum::http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("json"));
    if !is_json {
        return Value::Object(Map::new());
    }
    serde_json::from_slice(body).unwrap_or_else(|_| Value::Object(Map::new()))
}

/// A required list of paths: a 400 naming the field, like the TS `need(Paths, …)`.
fn need_paths(paths: Option<Vec<String>>) -> ApiResult<Vec<String>> {
    match paths {
        None => Err(HttpError::bad_request("paths: Invalid input: expected array, received undefined")),
        Some(p) if p.is_empty() => Err(HttpError::bad_request("paths: paths are required")),
        Some(p) => Ok(p),
    }
}

async fn op_route(
    State(s): State<AppState>,
    UrlPath((id, name)): UrlPath<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<GitOpResult>> {
    let cwd = root_path(&s, &id)?;
    let b = parse_op_body(lenient_body(&headers, &body))?;
    let text = |v: &Option<String>| v.clone().unwrap_or_default();
    let res = match name.as_str() {
        "fetch" => ops::fetch(&s, &cwd).await?,
        "pull" => ops::pull(&s, &cwd).await?,
        "push" => ops::push(&s, &cwd).await?,
        "commit" => ops::commit(&cwd, &text(&b.message), b.all.unwrap_or(true)).await,
        "stage" => ops::stage(&cwd, &need_paths(b.paths)?).await,
        "unstage" => ops::unstage(&cwd, &need_paths(b.paths)?).await,
        "checkout" => {
            let branch = crate::git_providers::js_trim(b.branch.as_deref().unwrap_or("")).to_string();
            if branch.is_empty() {
                return Err(HttpError::bad_request("branch: Branch is required"));
            }
            ops::checkout(&cwd, &branch, b.create.unwrap_or(false)).await
        }
        "branch-at" => {
            let name = crate::git_providers::js_trim(b.branch.as_deref().unwrap_or("")).to_string();
            ops::branch_at(&cwd, &name, &text(&b.rev)).await
        }
        "delete-branch" => ops::delete_branch(&cwd, &text(&b.branch), b.force.unwrap_or(false)).await,
        "merge" => ops::merge(&cwd, &text(&b.branch), b.no_ff.unwrap_or(false)).await,
        "rebase" => ops::rebase(&cwd, &text(&b.branch)).await,
        "reset" => ops::reset(&cwd, &text(&b.rev), b.mode.as_deref().unwrap_or("mixed")).await,
        "cherry-pick" => ops::cherry_pick(&cwd, &text(&b.rev)).await,
        "revert" => ops::revert(&cwd, &text(&b.rev)).await,
        "continue" | "abort" => ops::proceed(&cwd, &name).await,
        "discard" => ops::discard(&cwd, &need_paths(b.paths)?).await,
        "stash-push" => ops::stash_push(&cwd, &text(&b.message), b.untracked.unwrap_or(true)).await,
        "stash" => {
            let action = match b.action.as_deref() {
                Some(a @ ("apply" | "pop" | "drop")) => a,
                _ => return Err(HttpError::bad_request("action: action: apply | pop | drop")),
            };
            ops::stash(&cwd, action, &text(&b.rev)).await
        }
        _ => return Err(HttpError::not_found("Unknown git operation")),
    };
    Ok(Json(res))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_revs() {
        for ok in ["main", "feature/x", "HEAD~2", "stash@{0}", "v1.2^"] {
            assert!(valid_rev(ok), "{ok}");
        }
        for bad in ["", "-x", "a..b", "a b", "x;rm", &"a".repeat(201)] {
            assert!(!valid_rev(bad), "{bad}");
        }
    }

    #[test]
    fn op_body_like_the_zod_schema() {
        assert!(parse_op_body(json!({})).is_ok());
        assert_eq!(parse_op_body(json!({ "all": "yes" })).err().unwrap().message, "all: Invalid input: expected boolean, received string");
        assert_eq!(
            parse_op_body(json!({ "paths": [1] })).err().unwrap().message,
            "paths.0: Invalid input: expected string, received number"
        );
        assert!(parse_op_body(json!({ "mode": "x" })).is_err());
        assert!(parse_op_body(json!([])).is_err());
        let b = parse_op_body(json!({ "message": "m", "noFf": true, "extra": 1 })).unwrap();
        assert_eq!((b.message.as_deref(), b.no_ff), (Some("m"), Some(true)));
    }

    #[test]
    fn query_numbers_like_javascript() {
        assert_eq!(js_number_of(Some("10")), 10.0);
        assert_eq!(js_number_of(Some("")), 0.0);
        assert!(js_number_of(Some("abc")).is_nan());
        assert_eq!(js_num(151.0), "151");
    }
}
