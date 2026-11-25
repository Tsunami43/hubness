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
pub const KINDS: &[&str] = &["claude-code", "codex", "opencode", "grok"];

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

/// Signal a whole process group (agents run in their own group so stop kills the tree).
pub fn kill_group(pid: u32, signal: i32) {
    // SAFETY: kill(2) with a negative pid only sends a signal; no memory is touched
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}
