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
use std::sync::OnceLock;

use hubness_core::{Project, ProjectKind};
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;

use crate::config::Config;
use crate::db::{groups, projects};
use crate::paths;

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
    let s = |v: &str| v.to_string();
    // own PID namespace: when the agent exits, no processes are left inside; without a
    // controlling terminal the agent can't inject input into another tty (TIOCSTI)
    let mut args: Vec<String> = [
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--unshare-pid",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
        "--die-with-parent",
        "--new-session",
    ]
    .map(s)
    .to_vec();
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
