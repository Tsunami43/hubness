//! Agents known to the hub. For now only the registry of kinds, used to validate session
//! configs; the adapters themselves (`apps/server/src/agents/`) are not ported yet.

use hubness_core::{AgentConfig, PermissionMode};

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
