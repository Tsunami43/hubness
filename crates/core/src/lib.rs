//! Types of the hubness HTTP API, shared by the Rust server and Rust clients.
//!
//! The JSON shape mirrors `packages/shared` (the TypeScript types the web app uses):
//! camelCase field names, string enums, `null` for absent optional values.

use serde::{Deserialize, Serialize};

/// Error body of every failed API request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiError {
    pub error: String,
}

/// Permission mode a session runs with.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Ask,
    AcceptEdits,
    Bypass,
    Plan,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::AcceptEdits => "acceptEdits",
            Self::Bypass => "bypass",
            Self::Plan => "plan",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ask" => Self::Ask,
            "acceptEdits" => Self::AcceptEdits,
            "bypass" => Self::Bypass,
            "plan" => Self::Plan,
            _ => return None,
        })
    }
}

/// Repository git hosting. `Generic` is any other git server (no web links).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GitProviderId {
    Github,
    Gitlab,
    Bitbucket,
    Generic,
}

impl GitProviderId {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "github" => Self::Github,
            "gitlab" => Self::Gitlab,
            "bitbucket" => Self::Bitbucket,
            "generic" => Self::Generic,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProjectKind {
    /// A git repository added to the hub.
    Normal,
    /// The system project `home`.
    System,
}

impl ProjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::System => "system",
        }
    }
}

/// Primary language of a repository (cached from git, for lists).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Language {
    pub name: String,
    pub color: Option<String>,
}

/// A project is exactly one git repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    /// Repository folder (agent cwd).
    pub path: String,
    pub kind: ProjectKind,
    pub description: String,
    /// Instructions for AI agents (supplement the repository's AGENTS.md).
    pub instructions: String,
    pub git_provider: Option<GitProviderId>,
    /// Remote origin URL (without credentials).
    pub repo_url: Option<String>,
    pub default_branch: Option<String>,
    pub group_ids: Vec<String>,
    pub language: Option<Language>,
    /// The repository folder disappeared from disk (only in the project list).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<bool>,
    pub created_at: String,
    pub updated_at: String,
}

/// Logical group of projects: a product, a system, a set of microservices.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGroup {
    pub id: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    /// Storage folder for the group's new projects; `None` — the shared projects folder.
    pub folder: Option<String>,
    /// Member projects in order.
    pub project_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupDetail {
    #[serde(flatten)]
    pub group: ProjectGroup,
    pub projects: Vec<Project>,
    pub workspace: Workspace,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceScopeKind {
    Project,
    Group,
}

impl WorkspaceScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Group => "group",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceStrategy {
    /// One repository.
    Single,
    /// Several repositories of a group.
    Multi,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceStatus {
    Ready,
    Missing,
    Error,
}

/// Workspace provider capabilities — the UI shows only what is supported.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceCapabilities {
    pub lifecycle: bool,
    pub clones: bool,
}

/// Execution environment: where the code lives and where the agent and tools run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub scope: WorkspaceScopeKind,
    pub scope_id: String,
    pub provider: String,
    pub strategy: WorkspaceStrategy,
    pub path: String,
    pub status: WorkspaceStatus,
    pub status_detail: Option<String>,
    pub capabilities: WorkspaceCapabilities,
    pub created_at: String,
    pub updated_at: String,
}

/// Hub settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HubSettings {
    pub projects_root: String,
    pub default_permission: PermissionMode,
    pub default_instructions: String,
    pub session_instructions: String,
}

/// A repository found on disk that is not yet a hub project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveredRepo {
    pub path: String,
    pub name: String,
}

/// `GET /api/projects/discover`: import suggestions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoverResult {
    pub root: String,
    pub repos: Vec<DiscoveredRepo>,
}

// ---------- sessions ----------

/// Agent of a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub kind: String,
    /// Model id in the agent's own format. Absent — the agent's default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    /// Legacy Claude Code profile of old sessions. Absent — the default account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Idle,
    Running,
    Waiting,
    Error,
}

impl SessionStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "error" => Self::Error,
            _ => Self::Idle,
        }
    }
}

/// A session has exactly one scope: a project, a group, or a plain chat (both `None`,
/// an Inbox folder as `cwd`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub project_id: Option<String>,
    pub group_id: Option<String>,
    pub workspace_id: Option<String>,
    pub card_id: Option<String>,
    pub title: String,
    pub agent: AgentConfig,
    pub agent_session_id: Option<String>,
    pub cwd: Option<String>,
    pub status: SessionStatus,
    pub archived_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// An event of the session feed. Kept as raw JSON (`{uid, seq, ts, kind, ...}`, see
/// `HubEvent` in packages/shared) until the agent runtime is ported.
pub type HubEvent = serde_json::Value;

/// What the agent showed next to the chat (the `open_pane` MCP tool).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Pane {
    pub kind: PaneKind,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PaneKind {
    Url,
    File,
    Diff,
}

/// Messages of `GET /api/sessions/:id/stream`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
// short-lived messages: boxing the session would only add allocations
#[allow(clippy::large_enum_variant)]
pub enum SessionStreamMsg {
    Snapshot { session: Session, events: Vec<HubEvent>, pane: Option<Pane> },
    Pane { pane: Option<Pane> },
    Event { event: HubEvent },
    Session { session: Session },
}

/// Messages of `GET /api/stream` (for lists).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
// short-lived messages: boxing the session would only add allocations
#[allow(clippy::large_enum_variant)]
pub enum GlobalStreamMsg {
    #[serde(rename = "session")]
    Session { session: Session },
    #[serde(rename = "session-deleted")]
    SessionDeleted { id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_serializes_like_the_typescript_type() {
        let p = Project {
            id: "1".into(),
            name: "a".into(),
            path: "/a".into(),
            kind: ProjectKind::Normal,
            description: String::new(),
            instructions: String::new(),
            git_provider: Some(GitProviderId::Github),
            repo_url: None,
            default_branch: None,
            group_ids: vec![],
            language: None,
            missing: None,
            created_at: "t".into(),
            updated_at: "t".into(),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["gitProvider"], "github");
        assert_eq!(v["groupIds"], serde_json::json!([]));
        assert!(v.get("missing").is_none(), "missing is omitted unless set");
        assert_eq!(v["repoUrl"], serde_json::Value::Null);
    }

    #[test]
    fn stream_messages_are_tagged_like_the_typescript_unions() {
        let v = serde_json::to_value(GlobalStreamMsg::SessionDeleted { id: "x".into() }).unwrap();
        assert_eq!(v, serde_json::json!({ "type": "session-deleted", "id": "x" }));
        let v = serde_json::to_value(SessionStreamMsg::Pane { pane: None }).unwrap();
        assert_eq!(v, serde_json::json!({ "type": "pane", "pane": null }));
    }

    #[test]
    fn agent_config_omits_absent_fields() {
        let a: AgentConfig = serde_json::from_str(r#"{"kind":"codex","permissionMode":"plan"}"#).unwrap();
        assert_eq!(serde_json::to_string(&a).unwrap(), r#"{"kind":"codex","permissionMode":"plan"}"#);
    }

    #[test]
    fn permission_mode_round_trips() {
        for m in [PermissionMode::Ask, PermissionMode::AcceptEdits, PermissionMode::Bypass, PermissionMode::Plan] {
            assert_eq!(serde_json::to_value(m).unwrap(), m.as_str());
            assert_eq!(PermissionMode::parse(m.as_str()), Some(m));
        }
    }
}
