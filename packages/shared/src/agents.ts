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
