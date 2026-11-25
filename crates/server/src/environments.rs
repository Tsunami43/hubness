//! Where the agent runs — the agent part of `environments.ts`.
//!
//! **sandbox** (bubblewrap): the system is read-only; the projects folder, other projects,
//! hub data, keys (~/.ssh, ~/.aws…) and docker.sock are hidden; only the scope's
//! repositories and agent state folders are writable. The network is shared (agents need
//! the model API). On by default for projects when bubblewrap is available; groups are
//! sandboxed whenever it is available; the system project never is.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Path as UrlPath, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use hubness_core::{
    ContainerState, EnvironmentStatus, Project, ProjectContainer, ProjectGroup, ProjectKind, ProjectRuntime, RuntimeEnvironment,
};
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;

use crate::config::Config;
use crate::db::{groups, projects};
use crate::error::{ApiResult, HttpError};
use crate::http::{JsonBody, blocking};
use crate::terminal::shq;
use crate::{AppState, catalog, paths};

pub fn sandbox_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        Command::new("bwrap")
            .args(["--ro-bind", "/", "/", "--dev", "/dev", "true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// Saved project environment settings (`projects.runtime` JSON); only explicitly set fields.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedRuntime {
    pub environment: Option<String>,
    pub image: Option<String>,
    pub sandbox: Option<bool>,
    pub share_secrets_with_agents: Option<bool>,
}

pub fn saved_runtime(db: &Connection, project_id: &str) -> rusqlite::Result<SavedRuntime> {
    let raw: Option<Option<String>> = db.query_row("SELECT runtime FROM projects WHERE id = ?", [project_id], |r| r.get(0)).optional()?;
    Ok(raw.flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

/// Is the project sandboxed (saved setting, or on by default when bubblewrap is available)?
pub fn project_sandbox(db: &Connection, p: &Project) -> rusqlite::Result<bool> {
    if p.kind != ProjectKind::Normal {
        return Ok(false); // the system project (questions about the server) runs without a sandbox
    }
    Ok(saved_runtime(db, &p.id)?.sandbox.unwrap_or_else(sandbox_available))
}

fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

/// Agent state folders — writable (login, history, caches) for every registered agent.
fn agent_state(config: &Config) -> Vec<PathBuf> {
    let h = home();
    let codex = std::env::var("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|_| h.join(".codex"));
    let mut out: Vec<PathBuf> = [".cache", ".npm", ".bun"].iter().map(|d| h.join(d)).collect();
    // claude-code
    out.extend([h.join(".claude"), h.join(".claude.json"), config.data_dir.join("claude-profiles")]);
    // codex, opencode, grok
    out.push(codex);
    out.extend([".config/opencode", ".local/share/opencode", ".local/state/opencode"].iter().map(|d| h.join(d)));
    out.push(h.join(".grok"));
    out
}

fn uid() -> u32 {
    // the owner of our own /proc entry is the process's uid
    std::fs::metadata("/proc/self").map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap_or(0)
}

/// What to hide: other projects, hub data, keys and credentials, the Docker socket.
fn hidden_paths(db: &Connection, config: &Config, scope_repos: &[String]) -> rusqlite::Result<Vec<PathBuf>> {
    let in_scope = |p: &str| scope_repos.iter().any(|r| r == p || r.starts_with(&format!("{p}/")) || p.starts_with(&format!("{r}/")));
    let h = home();
    let mut list: Vec<PathBuf> = vec![config.projects_dir.clone()];
    list.extend(groups::list(db)?.into_iter().filter_map(|g| g.folder).map(PathBuf::from));
    list.extend(
        projects::list(db)?.into_iter().filter(|p| p.kind == ProjectKind::Normal && !in_scope(&p.path)).map(|p| PathBuf::from(p.path)),
    );
    list.push(config.data_dir.clone());
    list.push(config.hub_home.clone());
    list.push(paths::resolve(".env"));
    for d in [".ssh", ".gnupg", ".aws", ".azure", ".kube", ".docker", ".password-store", ".netrc", ".git-credentials", ".mozilla", ".pki"] {
        list.push(h.join(d));
    }
    for d in [".config/google-chrome", ".config/chromium", ".config/BraveSoftware", ".config/gh", ".config/gcloud", ".local/share/keyrings"]
    {
        list.push(h.join(d));
    }
    list.extend([PathBuf::from("/var/run/docker.sock"), PathBuf::from("/run/docker.sock"), PathBuf::from(format!("/run/user/{}", uid()))]);
    let mut seen = HashSet::new();
    let existing: Vec<PathBuf> = list.into_iter().filter(|p| p.exists() && seen.insert(p.clone())).collect();
    // don't mount folders nested in an already hidden one: the mount point would reveal another project's name
    let nested = |p: &Path| existing.iter().any(|q| q != p && p.starts_with(q));
    Ok(existing.iter().filter(|p| !nested(p)).cloned().collect())
}

/// bwrap arguments for an agent run: the scope's repositories and cwd are writable,
/// everything else read-only, secrets hidden.
pub fn bwrap_args(db: &Connection, config: &Config, repos: &[String], cwd: &str) -> rusqlite::Result<Vec<String>> {
    bwrap_args_for(db, config, repos, cwd, false)
}

/// bwrap arguments; `interactive` (Run and Shell on a pty) keeps the controlling terminal and
/// gives the shell its own history folder.
pub fn bwrap_args_for(db: &Connection, config: &Config, repos: &[String], cwd: &str, interactive: bool) -> rusqlite::Result<Vec<String>> {
    let s = |v: &str| v.to_string();
    // own PID namespace: when the agent exits, no processes are left inside; without a
    // controlling terminal the agent can't inject input into another tty (TIOCSTI)
    let mut args: Vec<String> =
        ["--ro-bind", "/", "/", "--dev", "/dev", "--unshare-pid", "--proc", "/proc", "--tmpfs", "/tmp", "--die-with-parent"]
            .map(s)
            .to_vec();
    if !interactive {
        args.push(s("--new-session"));
    }
    for p in hidden_paths(db, config, repos)? {
        let shown = paths::to_string(&p);
        if p.is_dir() {
            args.extend([s("--tmpfs"), shown]);
        } else {
            args.extend([s("--ro-bind"), s("/dev/null"), shown]);
        }
    }
    for p in agent_state(config) {
        if p.exists() {
            let shown = paths::to_string(&p);
            args.extend([s("--bind"), shown.clone(), shown]);
        }
    }
    // interactive shell history is separate (the host one with typed commands is not visible)
    if interactive {
        let hist = config.data_dir.join("shell-history");
        for (dir, target) in [("fish", ".local/share/fish"), ("zsh", ".local/share/zsh")] {
            let target = home().join(target);
            // a mount point can't be created on a read-only system — only existing folders
            if !target.exists() {
                continue;
            }
            let dir = hist.join(dir);
            let _ = std::fs::create_dir_all(&dir);
            args.extend([s("--bind"), paths::to_string(&dir), paths::to_string(&target)]);
        }
        // bound as is (HISTFILE lives there): it must exist even without fish/zsh folders
        let _ = std::fs::create_dir_all(&hist);
        let h = paths::to_string(&hist);
        args.extend([s("--bind"), h.clone(), h, s("--setenv"), s("HISTFILE"), paths::to_string(&hist.join("bash_history"))]);
    }
    for r in repos {
        args.extend([s("--bind"), r.clone(), r.clone()]);
    }
    if !repos.iter().any(|r| r == cwd) {
        args.extend([s("--bind"), s(cwd), s(cwd)]);
    }
    args.extend([s("--chdir"), s(cwd)]);
    Ok(args)
}

/// How to launch an agent: as is, or wrapped in bwrap.
#[derive(Debug, Clone, Default)]
pub struct Wrap {
    pub bwrap: Option<Vec<String>>,
}

impl Wrap {
    pub fn apply(&self, cmd: &str, args: Vec<String>) -> (String, Vec<String>) {
        match &self.bwrap {
            None => (cmd.to_string(), args),
            Some(b) => {
                let mut all = b.clone();
                all.push("--".into());
                all.push(cmd.into());
                all.extend(args);
                ("bwrap".into(), all)
            }
        }
    }
}

// ---------- Run, Shell and the project environment ----------
//
// - **local** — processes on the server, in the repository folder (sandboxed when the
//   project's sandbox setting is on).
// - **docker** — Run and Shell in the project container (modeled on Coder: the container is
//   disposable, home is a persistent folder; not root; no docker.sock; labels with the
//   project id). The agent stays on the server (CLI and login live there), sandboxed.

const DEFAULT_IMAGE: &str = "node:24-bookworm";

/// Run a short command: exit status and stdout, or `None` if it failed to start or timed out.
fn run_sync(cmd: &str, args: &[&str], timeout: Duration) -> Option<(bool, String)> {
    let mut child = Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    Some((status.success(), reader.join().unwrap_or_default()))
}

pub fn docker_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| run_sync("docker", &["version", "--format", "{{.Server.Version}}"], Duration::from_secs(5)).is_some_and(|r| r.0))
}

/// JSONC → JSON: comments and trailing commas outside strings (// inside "https://…" is left alone).
pub fn strip_jsonc(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '"' {
            let mut j = i + 1;
            while j < c.len() && c[j] != '"' {
                j += if c[j] == '\\' { 2 } else { 1 };
            }
            out.extend(&c[i..(j + 1).min(c.len())]);
            i = j;
        } else if c[i] == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            out.push('\n');
        } else if c[i] == '/' && c.get(i + 1) == Some(&'*') {
            let end = (i + 2..c.len().saturating_sub(1)).find(|&k| c[k] == '*' && c[k + 1] == '/');
            i = end.map(|e| e + 1).unwrap_or(c.len());
        } else {
            out.push(c[i]);
        }
        i += 1;
    }
    static TRAILING: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r",(\s*[}\]])").unwrap());
    TRAILING.replace_all(&out, "$1").into_owned()
}

/// Image from .devcontainer/devcontainer.json (image field only; Dockerfile builds are not supported).
pub fn devcontainer_image(dir: &str) -> Option<String> {
    for f in [".devcontainer/devcontainer.json", ".devcontainer.json"] {
        let Ok(text) = std::fs::read_to_string(Path::new(dir).join(f)) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&strip_jsonc(&text)) else { continue };
        if let Some(image) = v.get("image").and_then(|i| i.as_str()).map(str::trim).filter(|i| !i.is_empty()) {
            return Some(image.to_string());
        }
    }
    None
}

pub fn runtime_of(db: &Connection, p: &Project) -> rusqlite::Result<ProjectRuntime> {
    let saved = saved_runtime(db, &p.id)?;
    Ok(ProjectRuntime {
        environment: if saved.environment.as_deref() == Some("docker") { RuntimeEnvironment::Docker } else { RuntimeEnvironment::Local },
        image: saved
            .image
            .map(|i| i.trim().to_string())
            .filter(|i| !i.is_empty())
            .or_else(|| devcontainer_image(&p.path))
            .unwrap_or_else(|| DEFAULT_IMAGE.into()),
        // the sandbox is on by default if bubblewrap is available
        sandbox: saved.sandbox.unwrap_or_else(sandbox_available),
        share_secrets_with_agents: saved.share_secrets_with_agents.unwrap_or(false),
    })
}

/// Fields of `PUT /projects/:id/environment` (checked by `set_runtime`).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeFields {
    pub environment: Option<serde_json::Value>,
    pub image: Option<serde_json::Value>,
    pub sandbox: Option<serde_json::Value>,
    pub share_secrets_with_agents: Option<serde_json::Value>,
}

static IMAGE_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)^[a-z0-9][a-z0-9._/:@-]{0,254}$").unwrap());

fn flag(v: &serde_json::Value, name: &str) -> ApiResult<bool> {
    v.as_bool().ok_or_else(|| HttpError::bad_request(format!("{name}: {name} must be a boolean")))
}

/// Only explicitly set fields are saved — defaults (the devcontainer image) stay live.
pub fn set_runtime(db: &Connection, p: &Project, b: &RuntimeFields) -> ApiResult<ProjectRuntime> {
    let raw: Option<Option<String>> = db.query_row("SELECT runtime FROM projects WHERE id = ?", [&p.id], |r| r.get(0)).optional()?;
    let mut saved: serde_json::Map<String, serde_json::Value> =
        raw.flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if let Some(e) = &b.environment {
        let e = e.as_str().filter(|e| *e == "local" || *e == "docker");
        let Some(e) = e else { return Err(HttpError::bad_request("environment: environment must be local or docker")) };
        if e == "docker" && !docker_available() {
            return Err(HttpError::bad_request("Docker is not available on this server"));
        }
        saved.insert("environment".into(), e.into());
    }
    if let Some(i) = &b.image {
        let Some(i) = i.as_str() else { return Err(HttpError::bad_request("image: Invalid input: expected string")) };
        if i.encode_utf16().count() > 255 {
            return Err(HttpError::bad_request("image: Too big: expected string to have <=255 characters"));
        }
        let image = i.trim();
        if !image.is_empty() && !IMAGE_RE.is_match(image) {
            return Err(HttpError::bad_request("Invalid image name"));
        }
        // empty or equal to the default image — don't pin it
        if image.is_empty() || image == devcontainer_image(&p.path).as_deref().unwrap_or(DEFAULT_IMAGE) {
            saved.remove("image");
        } else {
            saved.insert("image".into(), image.into());
        }
    }
    if let Some(v) = &b.sandbox {
        let on = flag(v, "sandbox")?;
        if on && !sandbox_available() {
            return Err(HttpError::bad_request("bubblewrap (bwrap) is not installed on this server"));
        }
        saved.insert("sandbox".into(), on.into());
    }
    if let Some(v) = &b.share_secrets_with_agents {
        saved.insert("shareSecretsWithAgents".into(), flag(v, "shareSecretsWithAgents")?.into());
    }
    db.execute("UPDATE projects SET runtime = ? WHERE id = ?", (serde_json::Value::Object(saved).to_string(), &p.id))?;
    Ok(runtime_of(db, p)?)
}

// ---------- docker ----------

pub fn container_name(project_id: &str) -> String {
    format!("hubness-p-{}", project_id.chars().take(12).collect::<String>())
}

fn container_home(config: &Config, project_id: &str) -> PathBuf {
    config.data_dir.join("containers").join(project_id).join("home")
}

pub fn container_state(project_id: &str) -> (ContainerState, Option<String>) {
    if !docker_available() {
        return (ContainerState::Absent, None);
    }
    let name = container_name(project_id);
    match run_sync("docker", &["inspect", "--format", "{{.State.Running}} {{.Config.Image}}", &name], Duration::from_secs(5)) {
        Some((true, out)) => {
            let mut it = out.trim().split(' ');
            let running = it.next() == Some("true");
            (if running { ContainerState::Running } else { ContainerState::Stopped }, it.next().map(str::to_string))
        }
        _ => (ContainerState::Absent, None),
    }
}

pub fn container_ip(project_id: &str) -> Option<String> {
    let name = container_name(project_id);
    let fmt = "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}";
    match run_sync("docker", &["inspect", "--format", fmt, &name], Duration::from_secs(5)) {
        Some((true, out)) => out.split_whitespace().next().map(str::to_string),
        _ => None,
    }
}

fn ids() -> (u32, u32) {
    // SAFETY: getuid/getgid never fail
    unsafe { (libc::getuid(), libc::getgid()) }
}

/// sh script: bring up the project container if it doesn't exist or the image changed.
/// Runs inside a terminal task — the image pull is visible in the output.
fn ensure_container_script(config: &Config, p: &Project, rt: &ProjectRuntime) -> String {
    let name = container_name(&p.id);
    let home = container_home(config, &p.id);
    let _ = std::fs::create_dir_all(&home);
    let (uid, gid) = ids();
    let run = [
        "docker run -d --init".to_string(),
        format!("--name {}", shq(&name)),
        format!("--label hubness.project={}", shq(&p.id)),
        format!("--user {uid}:{gid}"),
        "-e HOME=/home/hub".into(),
        format!("-v {}:/home/hub", shq(&paths::to_string(&home))),
        format!("-v {}:/workspace", shq(&p.path)),
        "-w /workspace".into(),
        "--add-host host.docker.internal:host-gateway".into(),
        shq(&rt.image),
        "sleep infinity".into(),
    ]
    .join(" ");
    let (n, img) = (shq(&name), shq(&rt.image));
    [
        format!("cur=$(docker inspect --format '{{{{.Config.Image}}}}' {n} 2>/dev/null || true)"),
        format!(r#"if [ -n "$cur" ] && [ "$cur" != {img} ]; then echo "[hub] image changed: recreating container"; docker rm -f {n} >/dev/null; cur=; fi"#),
        format!(
            r#"if [ -z "$cur" ]; then echo "[hub] starting container {name} ({})"; {run} >/dev/null || exit 1; else docker start {n} >/dev/null || exit 1; fi"#,
            rt.image
        ),
    ]
    .join("\n")
}

pub fn stop_container(project_id: &str) {
    if docker_available() {
        let _ = run_sync("docker", &["rm", "-f", &container_name(project_id)], Duration::from_secs(30));
    }
}

/// Environment file (secrets) for docker exec: don't expose values in `ps`.
fn env_file(config: &Config, project_id: &str, env: &[(String, String)]) -> std::io::Result<String> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = config.data_dir.join("containers").join(project_id);
    std::fs::create_dir_all(&dir)?;
    let f = dir.join("exec.env");
    let body: String = env.iter().map(|(k, v)| format!("{k}={}\n", v.replace('\n', "\\n"))).collect();
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&f)?;
    std::io::Write::write_all(&mut file, body.as_bytes())?;
    Ok(paths::to_string(&f))
}

// ---------- commands for terminal tasks ----------

/// Where a terminal task runs: a project, or a group (its symlink workspace as cwd).
pub enum Scope {
    Project(Project),
    Group { group: ProjectGroup, projects: Vec<Project>, cwd: String },
}

impl Scope {
    fn repos(&self) -> Vec<String> {
        match self {
            Self::Project(p) => vec![p.path.clone()],
            Self::Group { projects, .. } => projects.iter().map(|p| p.path.clone()).collect(),
        }
    }
    fn cwd(&self) -> String {
        match self {
            Self::Project(p) => p.path.clone(),
            Self::Group { cwd, .. } => cwd.clone(),
        }
    }
    /// Key of the scope's shells.
    pub fn key(&self) -> String {
        match self {
            Self::Project(p) => format!("project:{}", p.id),
            Self::Group { group, .. } => format!("group:{}", group.id),
        }
    }
}

/// The login shell from the password database, else $SHELL, else bash; sh if it is missing.
fn user_shell() -> String {
    let mut sh: Option<String> = None;
    // SAFETY: getpwuid_r fills `pw` with pointers into `buf`, which outlives their use
    unsafe {
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        let mut buf = vec![0 as libc::c_char; 4096];
        if libc::getpwuid_r(libc::getuid(), &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) == 0 && !out.is_null() && !pw.pw_shell.is_null()
        {
            sh = Some(std::ffi::CStr::from_ptr(pw.pw_shell).to_string_lossy().into_owned()).filter(|s| !s.is_empty());
        }
    }
    let sh = sh.or_else(|| std::env::var("SHELL").ok().filter(|s| !s.is_empty())).unwrap_or_else(|| "/bin/bash".into());
    if Path::new(&sh).exists() { sh } else { "/bin/sh".into() }
}

/// A command line for a terminal task (run by bash inside a pty).
pub struct ExecSpec {
    pub command: String,
    pub cwd: String,
    pub env: Vec<(String, String)>,
    pub label: String,
    pub on_kill: Option<crate::terminal::Callback>,
}

/// Command line for a terminal task. `command = None` — interactive shell.
pub fn exec_command(
    db: &Connection,
    config: &Config,
    scope: &Scope,
    command: Option<&str>,
    env: Vec<(String, String)>,
    no_sandbox: bool,
) -> ApiResult<ExecSpec> {
    let cwd = scope.cwd();
    if let Scope::Project(p) = scope {
        let rt = runtime_of(db, p)?;
        if rt.environment == RuntimeEnvironment::Docker {
            let name = container_name(&p.id);
            let body = command.unwrap_or("if command -v bash >/dev/null; then exec bash -l; else exec sh -l; fi");
            // the process in the container doesn't die with the docker exec client — remember
            // its group (with -t it leads its own session) and kill it on stop
            let tag = format!("/tmp/hubness-{}.pid", &uuid::Uuid::new_v4().simple().to_string()[..8]);
            let inner = format!("echo $$ > {tag}; {body}");
            // preview connects to the container IP — the app must listen on 0.0.0.0 (many dev servers honor HOST)
            let mut file_env = vec![("HOST".to_string(), "0.0.0.0".to_string())];
            for (k, v) in env {
                file_env.retain(|(fk, _)| *fk != k);
                file_env.push((k, v));
            }
            let file = env_file(config, &p.id, &file_env)?;
            let exec = format!("docker exec -it --env-file {} -w /workspace {} sh -lc {}", shq(&file), shq(&name), shq(&inner));
            let kill_script = format!("kill -TERM -$(cat {tag}) 2>/dev/null; sleep 2; kill -KILL -$(cat {tag}) 2>/dev/null; rm -f {tag}");
            return Ok(ExecSpec {
                command: format!("{}\n{exec}", ensure_container_script(config, p, &rt)),
                cwd,
                env: vec![],
                label: format!("{} · docker {}", command.unwrap_or("shell"), rt.image),
                on_kill: Some(Box::new(move || {
                    let _ = Command::new("docker")
                        .args(["exec", &name, "sh", "-c", &kill_script])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .map(|mut c| std::thread::spawn(move || c.wait()));
                })),
            });
        }
    }
    let shell = user_shell();
    let inner = match command {
        None => format!("exec {} -l", shq(&shell)),
        Some(c) => format!("exec bash -lc {}", shq(c)),
    };
    let sandboxed = match scope {
        Scope::Project(p) => project_sandbox(db, p)?,
        // a group is sandboxed if bubblewrap is available (it has no environment settings of its own)
        Scope::Group { .. } => sandbox_available(),
    };
    let label = command.map(str::to_string).unwrap_or(shell);
    if sandboxed && !no_sandbox {
        let args = bwrap_args_for(db, config, &scope.repos(), &cwd, true)?;
        let args: Vec<String> = args.iter().map(|a| shq(a)).collect();
        return Ok(ExecSpec {
            command: format!("exec bwrap {} -- bash -c {}", args.join(" "), shq(&inner)),
            cwd,
            env,
            label: format!("{label} · sandbox"),
            on_kill: None,
        });
    }
    Ok(ExecSpec { command: inner, cwd, env, label, on_kill: None })
}

pub fn environment_status(rt: ProjectRuntime, p: &Project) -> EnvironmentStatus {
    let container = (rt.environment == RuntimeEnvironment::Docker).then(|| {
        let (state, image) = container_state(&p.id);
        ProjectContainer { name: container_name(&p.id), state, image }
    });
    EnvironmentStatus {
        runtime: rt,
        docker_available: docker_available(),
        sandbox_available: sandbox_available(),
        container,
        devcontainer_image: devcontainer_image(&p.path),
    }
}

// ---------- routes ----------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/projects/{id}/environment", get(get_environment).put(put_environment))
        .route("/projects/{id}/environment/container/remove", post(remove_container))
}

async fn get_environment(State(s): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Json<EnvironmentStatus>> {
    blocking(move || {
        let (p, rt) = {
            let db = s.db()?;
            let p = catalog::project_or_404(&db, &id)?;
            let rt = runtime_of(&db, &p)?;
            (p, rt)
        };
        Ok(environment_status(rt, &p))
    })
    .await
}

async fn put_environment(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
    JsonBody(b): JsonBody<RuntimeFields>,
) -> ApiResult<Json<EnvironmentStatus>> {
    blocking(move || {
        let db = s.db()?;
        let p = catalog::project_or_404(&db, &id)?;
        let before = runtime_of(&db, &p)?;
        let after = set_runtime(&db, &p, &b)?;
        // environment changed: running processes use the old one, so stop them
        let changed = before.environment != after.environment || before.image != after.image || before.sandbox != after.sandbox;
        if changed {
            crate::runs::stop_project_processes(&db, &p.id)?;
        }
        drop(db);
        if changed && before.environment == RuntimeEnvironment::Docker {
            stop_container(&p.id);
        }
        Ok(environment_status(after, &p))
    })
    .await
}

async fn remove_container(State(s): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Json<EnvironmentStatus>> {
    blocking(move || {
        let (p, rt) = {
            let db = s.db()?;
            let p = catalog::project_or_404(&db, &id)?;
            crate::runs::stop_project_processes(&db, &p.id)?;
            let rt = runtime_of(&db, &p)?;
            (p, rt)
        };
        stop_container(&p.id);
        Ok(environment_status(rt, &p))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn has_pair(args: &[String], flag: &str, a: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == a)
    }

    #[test]
    fn sandbox_hides_other_projects_and_binds_the_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cfg = Config::new(0, "127.0.0.1".into(), &root.join("data"), &root.join("hub"), &root.join("web"));
        cfg.ensure_dirs().unwrap();
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let mine = root.join("work/mine");
        let other = root.join("work/other");
        for d in [&mine, &other] {
            std::fs::create_dir_all(d.join(".git")).unwrap();
            projects::create(&conn, "p", &paths::to_string(d), ProjectKind::Normal).unwrap();
        }
        let mine_s = paths::to_string(&mine);
        let args = bwrap_args(&conn, &cfg, std::slice::from_ref(&mine_s), &mine_s).unwrap();
        assert!(has_pair(&args, "--tmpfs", &paths::to_string(&other)), "another project is hidden");
        assert!(!has_pair(&args, "--tmpfs", &mine_s), "the scope's repository is not hidden");
        assert!(has_pair(&args, "--tmpfs", &paths::to_string(&cfg.data_dir)), "hub data is hidden");
        assert!(has_pair(&args, "--bind", &mine_s) && has_pair(&args, "--chdir", &mine_s));
        assert!(args.contains(&"--new-session".to_string()) && args.contains(&"--die-with-parent".to_string()));
        // a hidden folder nested in another hidden one is not mounted (it would reveal its name)
        assert!(has_pair(&args, "--tmpfs", &paths::to_string(&cfg.hub_home)));
        assert!(!has_pair(&args, "--tmpfs", &paths::to_string(&cfg.projects_dir)), "projects/ is inside the hidden hub home");

        if sandbox_available() {
            let (cmd, full) = Wrap { bwrap: Some(args) }.apply("sh", vec!["-c".into(), "test ! -e ../other/.git && touch ok".into()]);
            let st = Command::new(cmd).args(full).status().unwrap();
            assert!(st.success(), "the other project is invisible inside, the scope is writable");
            assert!(mine.join("ok").exists());
        }
    }
}
