// Agents (harness CLIs): kinds, configs, availability, Harness page data.

/** Agent kind: id of an agent module registered on the server (see /api/agents). */
export type AgentKind = string;

/**
 * How the agent asks for permission:
 * - ask: asks for edits and commands;
 * - acceptEdits: file edits without asking, commands with a prompt;
 * - bypass: asks nothing (except questions to the user);
 * - plan: only plans, changes nothing.
 */
export type PermissionMode = 'ask' | 'acceptEdits' | 'bypass' | 'plan';

export interface AgentConfig {
  kind: AgentKind;
  /** Model id in the agent's own format (alias, provider/model…). Empty — the agent's default model. */
  model?: string;
  permissionMode?: PermissionMode;
  /** Agent account (legacy Claude Code profiles of old sessions). Empty — default. */
  account?: string;
}

/**
 * How the UI offers models of an agent:
 * - options: a short list with labels (ModelOption), picked as chips;
 * - search: a long list of ids (provider/model) with search and free input.
 */
export type ModelStyle = 'options' | 'search';

export interface AgentInfo {
  kind: AgentKind;
  label: string;
  available: boolean;
  /** CLI version or the reason it is unavailable. */
  detail: string;
  /** Installed version (null — not installed or unknown). */
  version?: string | null;
  /** Signed in (null — not known without a full check, or not installed). */
  signedIn?: boolean | null;
  modelStyle: ModelStyle;
}

export interface AgentsResponse {
  /** Registered agents in UI order. */
  agents: AgentInfo[];
  /** Agent preselected for a new session. */
  defaultKind: AgentKind;
}

// ---- Harness: versions, subscriptions, accounts ----

/** Harness action: install, update, login, logout. */
export interface HarnessAction {
  id: 'install' | 'update' | 'login' | 'login-console' | 'logout';
  /** Command to show and copy. */
  command: string;
  /** false — needs root: the hub only shows the command. */
  runnable: boolean;
  /** true — in the built-in terminal (login via link/code, install progress). */
  interactive: boolean;
}

/** Harness account: the agent login, or a provider credential (accountMode: providers). */
export interface HarnessAccount {
  id: string;
  label: string;
  /** email, login method, etc. */
  detail: string | null;
  loggedIn: boolean;
  active: boolean;
  /** providers mode: provider id (openai, openrouter…). */
  provider?: string;
  providerName?: string;
}

export interface HarnessCommon {
  version: HarnessVersion;
  actions: HarnessAction[];
  accounts: HarnessAccount[];
}

export interface HarnessVersion {
  installed: string | null;
  latest: string | null;
  status: 'up-to-date' | 'update-available' | 'unknown' | 'not-installed';
  /** Where it was installed from and where we get the latest version (npm, pacman…). */
  source: string;
  updateCommand: string | null;
}

export interface LimitWindow {
  /** CLI limit kind (weekly_all, weekly_scoped:Fable, session…) or a rate-limit window id (seven_day, five_hour). */
  id: string;
  /** session (5-hour) | daily | weekly | monthly — from the CLI; absent for old window ids. */
  group?: string | null;
  /** What the limit covers besides "all models": a model (Fable) or a surface. */
  scope?: string | null;
  /** 0–100 */
  utilization: number;
  resetsAt: string | null;
}

/** A model the agent offers: id to pass to the agent, label and description as the agent shows them. */
export interface ModelOption {
  /** `default` stands for the agent's own default (no model passed). */
  id: string;
  label: string;
  description?: string;
  /** Full model id behind an alias (opus → claude-opus-5-5). */
  resolved?: string;
}

/** Agent login as the harness reports it; every field but loggedIn is optional detail. */
export interface HarnessAuth {
  loggedIn: boolean;
  /** claude.ai, api key, ChatGPT / auth.json… */
  method: string | null;
  /** Subscription / plan name to show (Max 20x, Pro…). */
  plan?: string | null;
  /** Extra plan detail (display name of the plan). */
  planDetail?: string | null;
  email?: string | null;
  org?: string | null;
  expiresAt?: string | null;
  /** Paid usage beyond the plan is enabled. */
  extraUsage?: boolean | null;
}

/**
 * One harness (agent CLI) as the Harness page shows it. The page renders by
 * capabilities (which fields are present), never by agent kind.
 */
export interface HarnessInfo extends HarnessCommon {
  kind: AgentKind;
  label: string;
  /** How the hub talks to the agent. */
  protocol: 'acp' | 'cli';
  /** The hub pins the agent (adapter) version: updates are a separate checked change. */
  pinned: boolean;
  auth: HarnessAuth | null;
  /**
   * single — one agent login (sign in / out);
   * providers — model provider credentials (connect more via login, switch / sign out each).
   */
  accountMode: 'single' | 'providers';
  /** Subscription limits; null — the agent doesn't report them, [] — none seen yet. */
  limits: LimitWindow[] | null;
  /** When the limits were obtained. */
  limitsAsOf: string | null;
  models: ModelOption[];
  /** The agent's own default model; null — its built-in default. */
  defaultModel: string | null;
  /** The default model can be set from the hub: options — pick one of models; search — any id; null — no. */
  defaultModelEdit: ModelStyle | null;
  /** Usage stats the CLI reports about itself. */
  usage: { sessions: number | null; prompts: number | null; tokens: string | null } | null;
}

export interface HarnessResponse {
  harness: HarnessInfo[];
  checkedAt: string;
}

export interface AgentModelsResponse {
  models: string[];
  /** Labels and descriptions, when the agent provides them (modelStyle: options). */
  options?: ModelOption[];
}

// ---- Harness: MCP servers and extensions (skills, rules, subagents, commands) ----

export type McpTransport = 'stdio' | 'http' | 'sse';
/** user — the agent's own user config; project — a project's config (shown, never written). */
export type McpScope = 'user' | 'project';

/** Where one agent has a server: its scope, file and whether the agent has it turned on. */
export interface McpEntry {
  agent: AgentKind;
  scope: McpScope;
  /** The config file the definition comes from. */
  source: string;
  enabled: boolean;
}

/**
 * One MCP server, as a row of the matrix. Secrets never leave the server: `env` and
 * `headers` list the variable / header names only.
 */
export interface McpServerInfo {
  name: string;
  transport: McpTransport;
  command: string | null;
  args: string[];
  /** Environment variable names (values are not sent). */
  env: string[];
  url: string | null;
  /** Header names (values are not sent). */
  headers: string[];
  /** The hub's own server, attached to hub sessions by the hub: read-only. */
  builtIn: boolean;
  /** Per agent: the scope it has the server in (user wins over project), builtin, or null. */
  agents: Record<AgentKind, McpScope | 'builtin' | null>;
  entries: McpEntry[];
}

export interface McpAgentInfo {
  kind: AgentKind;
  label: string;
  /** The hub can write this agent's MCP config. */
  supported: boolean;
  /** The user config file the hub writes (null when unsupported). */
  configPath: string | null;
  /** The transports the agent's config can hold. */
  transports: McpTransport[];
  /** Why it is unsupported, or why its config can't be read (it is then left alone). */
  error: string | null;
}

export interface McpResponse {
  servers: McpServerInfo[];
  agents: McpAgentInfo[];
}

/** A server to add to (or replace in) the selected agents' user configs. */
export interface McpServerInput {
  name: string;
  transport: McpTransport;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  url?: string;
  headers?: Record<string, string>;
  agents: AgentKind[];
}

/** user — the user's own folder; project — the project; plugin — an installed plugin. */
export type ExtensionScope = 'user' | 'project' | 'plugin';

export interface SkillInfo {
  name: string;
  description: string;
  path: string;
  scope: ExtensionScope;
}

/** A rules / memory file the agent reads (CLAUDE.md, AGENTS.md…), listed even when missing. */
export interface RulesFileInfo {
  label: string;
  path: string;
  scope: ExtensionScope;
  exists: boolean;
  size: number;
  /** The beginning of the file. */
  preview: string | null;
}

export interface SubagentInfo {
  name: string;
  description: string;
  model: string | null;
  path: string;
  scope: ExtensionScope;
}

export interface CommandInfo {
  name: string;
  description: string;
  path: string;
  scope: ExtensionScope;
}

export interface PluginInfo {
  name: string;
  version: string | null;
  enabled: boolean;
  path: string;
  scope: ExtensionScope;
}

/** What an agent is extended with, read from its own folders (and the project's). */
export interface HarnessExtensions {
  kind: AgentKind;
  skills: SkillInfo[];
  rules: RulesFileInfo[];
  agents: SubagentInfo[];
  commands: CommandInfo[];
  plugins: PluginInfo[];
}

export interface RulesFileContent {
  path: string;
  exists: boolean;
  content: string;
}

/** Every agent's extensions at once (GET /api/harness/extensions), agents in UI order. */
export interface HarnessExtensionsAll {
  agents: HarnessExtensions[];
}

// ---- Harness: MCP catalog (the official MCP Registry, proxied by the hub) ----

/** An environment variable or header an install option asks for. */
export interface McpCatalogVar {
  name: string;
  description: string;
  required: boolean;
  /** A secret: entered in a masked field, written only into the agents' configs. */
  secret: boolean;
  /** A prefilled value or template from the registry (e.g. `Bearer {token}`). */
  value: string | null;
}

/** One way to run a catalog server, ready for POST /api/harness/mcp. */
export interface McpInstallOption {
  kind: 'npm' | 'pypi' | 'docker' | 'remote';
  transport: McpTransport;
  command: string | null;
  args: string[];
  url: string | null;
  env: McpCatalogVar[];
  headers: McpCatalogVar[];
}

export interface McpCatalogEntry {
  /** The registry id (e.g. io.github.upstash/context7). */
  id: string;
  title: string;
  description: string;
  version: string | null;
  repository: string | null;
  website: string | null;
  /** A short config name for the server (letters, digits, - and _). */
  suggestedName: string;
  /** Installable options (stdio packages from npm / PyPI / Docker, and remotes); may be empty. */
  options: McpInstallOption[];
}

export interface McpCatalogResponse {
  entries: McpCatalogEntry[];
  /** Pass as ?cursor= for the next page (null — no more). */
  nextCursor: string | null;
  /** popular — the hub's built-in list (an empty search); registry — the MCP Registry. */
  source: 'popular' | 'registry';
}

// ---- Harness: skills across the agents ----

/** user — the agent's own skills folder (writable); project / plugin — shown, read-only. */
export type SkillScope = 'user' | 'project' | 'plugin';

export interface SkillEntry {
  agent: AgentKind;
  scope: SkillScope;
  /** The skill folder. */
  path: string;
}

/** A skill (a folder with SKILL.md, keyed by the folder name), as a row of the matrix. */
export interface SkillRow {
  name: string;
  description: string;
  /** Per agent: the scope it has the skill in (user wins over project, project over plugin), or null. */
  agents: Record<AgentKind, SkillScope | null>;
  entries: SkillEntry[];
}

export interface SkillAgentInfo {
  kind: AgentKind;
  label: string;
  /** The hub can write this agent's skills. */
  supported: boolean;
  /** The user skills folder the hub writes (null when unsupported). */
  dir: string | null;
}

export interface SkillsResponse {
  skills: SkillRow[];
  agents: SkillAgentInfo[];
}

export interface SkillFile {
  /** Relative to the skill folder. */
  path: string;
  size: number;
}

export interface SkillContent {
  name: string;
  agent: AgentKind;
  scope: SkillScope;
  /** The skill folder. */
  path: string;
  /** SKILL.md as text. */
  content: string;
  files: SkillFile[];
}

/** A new skill: SKILL.md with name / description frontmatter and the body. */
export interface SkillCreateInput {
  name: string;
  description: string;
  body: string;
  agents: AgentKind[];
  /** Replace an existing different skill of the same name (the old one goes to the hub trash). */
  replace?: boolean;
}

export interface SkillImportFound {
  name: string;
  description: string;
  /** The folder inside the repository. */
  path: string;
}

export interface SkillImportPreview {
  /** Pass to POST /api/harness/skills/import (valid for a limited time). */
  id: string;
  /** The URL that was cloned. */
  url: string;
  skills: SkillImportFound[];
}

export interface SkillImportInput {
  id: string;
  skills: string[];
  agents: AgentKind[];
  replace?: boolean;
}
