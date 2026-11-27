//! Types of the hubness HTTP API, shared by the Rust server and Rust clients.
//!
//! The JSON shape mirrors `packages/shared` (the TypeScript types the web app uses):
//! camelCase field names, string enums, `null` for absent optional values.

use serde::{Deserialize, Serialize};

mod system;
pub use system::*;

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
    /// The hub's data folder (database, logs, keys); set by HUBNESS_DATA_DIR.
    pub data_dir: String,
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

/// Serialize a float the way JavaScript prints numbers: integral values without `.0`.
pub fn js_number<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9_007_199_254_740_992.0 { s.serialize_i64(*v as i64) } else { s.serialize_f64(*v) }
}

/// Project manifest `.hub/hub.yaml`: links, SSH hosts, containers, units, documents.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// Name → URL, in file order.
    pub links: serde_json::Map<String, serde_json::Value>,
    pub ssh: Vec<String>,
    pub services: Vec<String>,
    pub units: Vec<String>,
    pub docs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceState {
    pub name: String,
    pub state: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestResponse {
    pub exists: bool,
    pub path: String,
    pub manifest: Option<Manifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub services: Vec<ServiceState>,
}

// ---------- files ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Ignored by git (.gitignore) or an internal folder (node_modules…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignored: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileContent {
    pub path: String,
    pub size: u64,
    pub binary: bool,
    pub truncated: bool,
    pub content: String,
    /// sha256 of the file bytes (text files that fit entirely) — base for a conflict-safe save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowseResponse {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<FsEntry>,
    pub is_git_repo: bool,
}

/// Files changed on disk in a project folder (`GET /api/watch`, batched).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeMsg {
    pub project_id: String,
    /// Changed paths relative to the project root ('/'-separated): files or folders.
    pub paths: Vec<String>,
    /// HEAD, index or refs changed (commit, checkout, staging).
    pub git: bool,
    /// Too many changes at once: reload everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overflow: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchMatch {
    /// 1-based line number.
    pub line: u64,
    /// The line (cut to a preview length).
    pub text: String,
    /// Match ranges inside `text` (UTF-16 indices, end exclusive).
    pub ranges: Vec<(usize, usize)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchFile {
    pub path: String,
    pub matches: Vec<SearchMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResult {
    pub files: Vec<SearchFile>,
    pub total: usize,
    pub truncated: bool,
}

/// A project secret: the value never leaves the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SecretInfo {
    pub name: String,
    pub updated_at: String,
}

// ---------- project repository info (packages/shared/src/projects.ts) ----------

/// A language of a repository by bytes (like the language bar on GitHub).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoLanguage {
    pub name: String,
    pub color: Option<String>,
    #[serde(serialize_with = "js_number")]
    pub percent: f64,
}

/// Provider web links of a repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepoLinks {
    pub repo: String,
    pub branch: Option<String>,
    pub new_pull_request: Option<String>,
}

/// Project repository info (read from git on request).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RepoInfo {
    /// Languages by bytes (colors from GitHub Linguist).
    pub languages: Vec<RepoLanguage>,
    pub is_repo: bool,
    pub provider: Option<GitProviderId>,
    pub provider_label: Option<String>,
    pub remote_url: Option<String>,
    pub branch: Option<String>,
    pub default_branch: Option<String>,
    /// Provider web links (`None` for generic).
    pub links: Option<RepoLinks>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupRef {
    pub id: String,
    pub name: String,
}

/// `GET /api/projects/:id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectDetail {
    #[serde(flatten)]
    pub project: Project,
    pub repo: RepoInfo,
    pub groups: Vec<GroupRef>,
    pub workspace: Workspace,
}

// ---------- git panel (packages/shared/src/git.ts) ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitFile {
    pub path: String,
    /// Index status letter (X from porcelain) or ' '.
    pub index: String,
    /// Working tree status letter (Y) or ' '.
    pub worktree: String,
    pub untracked: bool,
    pub conflicted: bool,
    /// For renames — the old path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// An operation stopped half-way (conflicts or a paused rebase).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GitRepoState {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl GitRepoState {
    /// The git command that continues or aborts this operation.
    pub fn command(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub detached: bool,
    pub files: Vec<GitFile>,
    pub state: Option<GitRepoState>,
    /// Number of stash entries.
    pub stashes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitStash {
    /// `stash@{0}`
    #[serde(rename = "ref")]
    pub reference: String,
    pub message: String,
    pub date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitBranch {
    pub name: String,
    pub current: bool,
    pub remote: bool,
    pub upstream: Option<String>,
    pub last_commit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitCommit {
    pub hash: String,
    pub short: String,
    /// Parent hashes (two or more — a merge).
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    pub date: String,
    /// Decorations from `%D`: `HEAD -> main`, `origin/main`, `tag: v1`.
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitLogResponse {
    pub commits: Vec<GitCommit>,
    /// More commits exist beyond the limit.
    pub more: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GitAuthProtocol {
    Https,
    Ssh,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GitAuthReason {
    /// No login stored for the host (https) or no key accepted (ssh).
    Missing,
    /// The stored login was refused.
    Rejected,
    /// The ssh host is not in known_hosts.
    Hostkey,
}

/// Why the remote refused access.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitAuthProblem {
    pub host: String,
    pub protocol: GitAuthProtocol,
    pub reason: GitAuthReason,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitOpResult {
    pub ok: bool,
    pub output: String,
    /// Set when a network operation failed because the remote refused access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<GitAuthProblem>,
}

impl GitOpResult {
    pub fn fail(output: impl Into<String>) -> Self {
        Self { ok: false, output: output.into(), auth: None }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GitCredentialKind {
    Token,
    Password,
}

impl GitCredentialKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Token => "token",
            Self::Password => "password",
        }
    }
}

/// Stored git login for one host (the secret itself never leaves the server).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitCredentialInfo {
    pub host: String,
    pub username: String,
    pub kind: GitCredentialKind,
    pub updated_at: String,
}

// ---------- SSH connections ----------

/// A server the hub can reach over SSH with a key it holds (Settings → SSH). The private key
/// never leaves the server; `public_key` goes into the server's `authorized_keys`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SshConnection {
    pub id: String,
    /// The alias agents use: `ssh <name>`.
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Free label: dev, staging, prod… (shown to people and agents).
    pub environment: String,
    /// What the server is for — the agent reads it to tell prod from dev.
    pub description: String,
    pub public_key: String,
    /// `known_hosts` lines pinned by the first successful test (empty until then).
    pub host_keys: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A connection attached to a project or a group, with what it is for there.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SshBinding {
    pub connection: SshConnection,
    pub note: String,
}

/// `POST /api/ssh/:id/test`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SshTestResult {
    pub ok: bool,
    pub output: String,
    pub connection: SshConnection,
}

// ---------- user accounts ----------

/// What an account may do: an admin everything (including managing accounts), a member only
/// what its `Permissions` allow.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    Admin,
    Member,
}

/// Parts of the hub a member can be given (chats in its projects are always available).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    /// The project's git client.
    Git,
    /// The file explorer and editor.
    Files,
    /// Run commands, shells, preview and the project environment.
    Terminal,
    /// The project board.
    Kanban,
    /// The host page (Docker, systemd, packages).
    System,
    /// Agents, MCP servers, skills and the agents' accounts.
    Harness,
    /// Hub-wide settings: defaults, git access, voice input.
    Settings,
}

/// A member's permissions. `None` lists mean "all".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    /// Projects the member can open (and groups made only of them); `None` — every project.
    #[serde(default)]
    pub projects: Option<Vec<String>>,
    #[serde(default)]
    pub sections: Vec<Section>,
    /// Agent kinds the member can start sessions with; `None` — every agent.
    #[serde(default)]
    pub agents: Option<Vec<String>>,
    /// Models the member can pick; `None` — every model.
    #[serde(default)]
    pub models: Option<Vec<String>>,
}

impl Default for Permissions {
    /// A new member: no projects yet, files and git, every agent and model.
    fn default() -> Self {
        Self { projects: Some(vec![]), sections: vec![Section::Files, Section::Git], agents: None, models: None }
    }
}

/// A hub account (the password hash never leaves the server). `permissions` is set for
/// members only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub username: String,
    pub created_at: String,
    #[serde(default)]
    pub role: Role,
    #[serde(default)]
    pub permissions: Option<Permissions>,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }
}

/// `GET /api/auth/status`: `required` — the hub has accounts and asks for a sign-in;
/// `setup` — no accounts yet (the first one can be created from this machine);
/// `user` — who is signed in with this request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub required: bool,
    pub setup: bool,
    pub user: Option<User>,
}

/// `POST /api/auth/login` (and `/api/auth/setup`): the session token for API clients
/// (`Authorization: Bearer`); browsers get the same session as an HttpOnly cookie.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub token: String,
    pub user: User,
}

// ---------- run commands, shells, environment ----------

/// Status of a terminal job.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Exited,
}

/// Terminal task (install, login, update, Run, Shell).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub id: String,
    pub title: String,
    pub command: String,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    pub started_at: String,
}

/// Messages of `GET /api/jobs/:id/stream`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum JobStreamMsg {
    Snapshot { job: JobInfo, output: String },
    Output { data: String },
    Exit { code: Option<i32> },
}

/// A named project command (Run).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub command: String,
    /// App port for preview; null — detect from the output (http://localhost:5173).
    pub port: Option<u16>,
    /// The primary command — the Run button starts it.
    pub primary: bool,
    /// Needs Docker (docker compose…): the command runs without the sandbox — the
    /// Docker socket is hidden inside it. Docker access ≈ root on the server.
    pub docker: bool,
    pub position: i64,
}

/// A run command with its current run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    #[serde(flatten)]
    pub config: RunConfig,
    /// Current or last run (terminal task).
    pub job: Option<JobInfo>,
    /// Port: from the setting or detected in the output.
    pub active_port: Option<u16>,
    /// Preview proxy port on the hub (same host as the hub).
    pub preview_port: Option<u16>,
}

/// Run command suggestion (package.json, Makefile, …).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunSuggestion {
    pub name: String,
    pub command: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker: Option<bool>,
}

/// Where Run and Shell execute: on the server or in the project container.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeEnvironment {
    Local,
    Docker,
}

/// Where Run and Shell execute and how the agent is isolated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRuntime {
    pub environment: RuntimeEnvironment,
    /// Container image (environment = docker).
    pub image: String,
    pub sandbox: bool,
    pub share_secrets_with_agents: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContainerState {
    Running,
    Stopped,
    Absent,
}

/// The project container (environment = docker).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectContainer {
    pub name: String,
    pub state: ContainerState,
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentStatus {
    pub runtime: ProjectRuntime,
    /// Availability of the mechanisms on the server.
    pub docker_available: bool,
    pub sandbox_available: bool,
    /// Container state (environment = docker).
    pub container: Option<ProjectContainer>,
    /// Image from .devcontainer/devcontainer.json, if present.
    pub devcontainer_image: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CardColumn {
    Todo,
    Doing,
    Review,
    Done,
}

impl CardColumn {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::Doing => "doing",
            Self::Review => "review",
            Self::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "doing" => Self::Doing,
            "review" => Self::Review,
            "done" => Self::Done,
            _ => Self::Todo,
        }
    }
}

/// A kanban card of a project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub column: CardColumn,
    #[serde(serialize_with = "js_number")]
    pub position: f64,
    pub session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
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

/// A file attached to a message, stored in the session folder (`.hub/attachments/`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attachment {
    /// File name in the attachments folder (unique).
    pub id: String,
    /// Original file name.
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// Absolute path on the server (what the agent is told).
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectSuggestion {
    pub suggest: bool,
    pub reason: Option<String>,
    pub name: String,
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

// ---------- voice input ----------

/// Where speech is transcribed: off, a local whisper.cpp, or an OpenAI-compatible service.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum VoiceProvider {
    #[default]
    Off,
    Local,
    Api,
}

impl VoiceProvider {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "off" => Self::Off,
            "local" => Self::Local,
            "api" => Self::Api,
            _ => return None,
        })
    }
}

/// A whisper.cpp (ggml) model the hub can download.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceModel {
    pub id: String,
    pub label: String,
    /// Download size in bytes (approximate).
    pub size: u64,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceLocal {
    pub model: String,
    pub binary: Option<String>,
    pub managed: bool,
    pub ffmpeg: bool,
    pub missing_build_tools: Vec<String>,
    pub models: Vec<VoiceModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceApi {
    pub url: String,
    pub model: String,
    pub has_key: bool,
}

/// Settings → Voice input (`GET /api/voice`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSettings {
    pub provider: VoiceProvider,
    pub language: String,
    pub ready: bool,
    pub local: VoiceLocal,
    pub api: VoiceApi,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptionResult {
    pub text: String,
}
