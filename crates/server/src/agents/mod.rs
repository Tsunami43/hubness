//! Agents known to the hub. The core talks to agents only through `start` and `RunHandle`
//! (the TS `AgentModule`/`AgentAdapter`): Claude Code (stream-json) and the ACP agents
//! Codex, OpenCode and Grok.

pub mod acp;
pub mod claude;

use std::sync::Arc;

use hubness_core::{AgentConfig, PermissionMode};
use serde::Deserialize;
use tokio::task::JoinHandle;

use crate::runtime::Turn;

/// Registered agent kinds, in the order of the TS registry.
pub const KINDS: &[&str] = &["opencode", "claude-code", "grok", "codex"];

pub fn is_known(kind: &str) -> bool {
    KINDS.contains(&kind)
}

/// Agent config of a new session: a registered agent; no mode → the hub default. New
/// sessions always use the agent's default account (legacy profiles only resume old ones).
pub fn validate(kind: &str, model: Option<&str>, mode: Option<PermissionMode>, default_mode: PermissionMode) -> Option<AgentConfig> {
    if !is_known(kind) {
        return None;
    }
    Some(AgentConfig {
        kind: kind.to_string(),
        model: model.map(str::trim).filter(|m| !m.is_empty()).map(String::from),
        permission_mode: Some(mode.unwrap_or(default_mode)),
        account: None,
    })
}

/// Why a turn was stopped: by the user (Stop) or by hub exit/restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    User,
    Shutdown,
}

/// Feed text about the stop.
pub fn stop_notice(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Shutdown => crate::db::sessions::INTERRUPTED_NOTICE,
        StopReason::User => "Stopped",
    }
}

/// The user's answer to a permission request.
#[derive(Debug, Clone, Deserialize)]
pub struct ApprovalDecision {
    pub decision: Decision,
    pub reason: Option<String>,
    pub answers: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
}

/// A running turn.
pub trait RunHandle: Send + Sync {
    fn stop(&self, reason: StopReason);
    /// Reply to a permission request. `false`: the request is not pending.
    fn respond(&self, request_id: &str, decision: ApprovalDecision) -> bool;
}

pub type Started = (Arc<dyn RunHandle>, JoinHandle<()>);

/// Start a turn with the session's agent. `Err` — a message for the feed.
pub fn start(kind: &str, turn: Arc<Turn>) -> Result<Started, String> {
    match kind {
        "claude-code" => claude::start(turn),
        "codex" => acp::start(&acp::CODEX, turn),
        "opencode" => acp::start(&acp::OPENCODE, turn),
        "grok" => acp::start(&acp::GROK, turn),
        k => Err(format!("Agent “{k}” is not supported anymore — start a new session")),
    }
}

/// A lazy command wrapper of mise (Omarchy puts `~/.local/share/mise/command-wrappers` first
/// on PATH): running one makes mise download and install its own copy of the tool.
pub fn is_lazy_wrapper(path: &std::path::Path) -> bool {
    path.to_string_lossy().contains("/mise/command-wrappers/")
}

/// Where `name` is on PATH: the first real binary (lazy mise wrappers skipped), or the wrapper
/// when that is all there is (`Err`), or nothing.
pub fn find_on_path(name: &str) -> Option<Result<std::path::PathBuf, std::path::PathBuf>> {
    find_in(&std::env::var("PATH").ok()?, name)
}

fn find_in(path: &str, name: &str) -> Option<Result<std::path::PathBuf, std::path::PathBuf>> {
    let mut wrapper = None;
    for dir in path.split(':').filter(|d| !d.is_empty()) {
        let p = std::path::Path::new(dir).join(name);
        if !p.is_file() {
            continue;
        }
        if is_lazy_wrapper(&p) {
            wrapper.get_or_insert(p);
        } else {
            return Some(Ok(p));
        }
    }
    wrapper.map(Err)
}

/// The agent CLI to run: the `env` override, else a real install found on PATH (preferred over a
/// lazy mise wrapper, which would install mise's own copy instead of using the user's), else
/// the bare name.
pub fn agent_bin(env: &str, name: &str) -> String {
    let chosen = std::env::var(env).ok().filter(|b| !b.trim().is_empty()).unwrap_or_else(|| name.to_string());
    resolve(&chosen)
}

/// A command name → the real install on PATH (see `agent_bin`); paths and unknown names as is.
pub fn resolve(name: &str) -> String {
    if name.contains('/') {
        return name.to_string();
    }
    match find_on_path(name) {
        Some(Ok(p)) => p.to_string_lossy().into_owned(),
        _ => name.to_string(),
    }
}

/// Signal a whole process group (agents run in their own group so stop kills the tree).
pub fn kill_group(pid: u32, signal: i32) {
    // SAFETY: kill(2) with a negative pid only sends a signal; no memory is touched
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_installs_win_over_lazy_mise_wrappers() {
        let dir = tempfile::tempdir().unwrap();
        let wrappers = dir.path().join("mise/command-wrappers/bin");
        let local = dir.path().join("bin");
        for d in [&wrappers, &local] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(wrappers.join("tool"), "").unwrap();
        std::fs::write(wrappers.join("only-wrapped"), "").unwrap();
        std::fs::write(local.join("tool"), "").unwrap();
        let path = format!("{}:{}", wrappers.display(), local.display());
        assert_eq!(find_in(&path, "tool"), Some(Ok(local.join("tool"))));
        assert_eq!(find_in(&path, "only-wrapped"), Some(Err(wrappers.join("only-wrapped"))));
        assert_eq!(find_in(&path, "missing"), None);
    }
}
