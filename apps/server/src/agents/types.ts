import type { AgentModelsResponse, ApprovalDecision, Attachment, ExtensionScope, HarnessInfo, HubEvent, ModelStyle, PluginInfo, Session, SessionStatus, SubagentInfo } from '@hubness/shared';
import type { McpStore } from './mcp-config.ts';

/** Event body without service fields: the runtime fills those in. */
export type EventBody = HubEvent extends infer E
  ? E extends HubEvent
    ? Omit<E, 'uid' | 'seq' | 'ts'>
    : never
  : never;

export interface McpServerSpec {
  name: string;
  command: string;
  args: string[];
  env: Record<string, string>;
}

export interface RunContext {
  session: Session;
  cwd: string;
  prompt: string;
  /** Files attached to this message (paths are already in `prompt`); images may go to the model directly. */
  attachments: Attachment[];
  /** Other project folders (repository group): the agent has access to all of them. */
  extraDirs: string[];
  /** Additional system context (project manifest). */
  systemPrompt: string | null;
  /** Hub MCP servers (open_pane). */
  mcpServers: McpServerSpec[];
  /** Extra environment for the agent process (project secrets, if allowed). */
  env: Record<string, string>;
  /** Launch wrapper (bubblewrap sandbox); null means launch as is. */
  wrap: ((cmd: string, args: string[]) => [string, string[]]) | null;
  /** Create or replace an event. persist=false: broadcast only (delta streaming). */
  put(uid: string, body: EventBody, opts?: { persist?: boolean }): void;
  /** Partially update an existing event. */
  patch(uid: string, patch: Partial<EventBody>, opts?: { persist?: boolean }): void;
  get(uid: string): HubEvent | undefined;
  setStatus(status: SessionStatus): void;
  setAgentSessionId(id: string): void;
  /** Full session history (for agents without their own memory). */
  history(): HubEvent[];
  /** Raw agent output, written to the run's log file. */
  raw(line: string): void;
}

/** Why a turn was stopped: by the user (Stop) or by hub exit/restart. */
export type StopReason = 'user' | 'shutdown';

/** Feed message about the stop. */
/** Feed notice for a run cut off by a hub restart (also added on startup to sessions left running). */
export const INTERRUPTED_NOTICE = 'Interrupted: hubness restarted. Send a message to continue.';

export const stopNotice = (reason: StopReason) => (reason === 'shutdown' ? INTERRUPTED_NOTICE : 'Stopped');

export interface RunHandle {
  stop(reason?: StopReason): void;
  /** Reply to a permission request. false: request not found. */
  respond(requestId: string, decision: ApprovalDecision): boolean;
  done: Promise<void>;
}

export interface AgentAdapter {
  start(ctx: RunContext): RunHandle;
}

/** Cheap availability check of an agent for pickers (the core caches it). */
export interface AgentStatus {
  available: boolean;
  /** CLI version or the reason it is unavailable. */
  detail: string;
  version: string | null;
  signedIn: boolean | null;
}

/**
 * Everything the hub knows about one agent. The core (runtime, routes, sandbox,
 * inbox, Harness page) talks to agents only through this interface: a new agent is
 * a new module registered in registry.ts — no core or UI changes.
 */
export interface AgentModule {
  kind: string;
  label: string;
  adapter: AgentAdapter;
  modelStyle: ModelStyle;
  status(): Promise<AgentStatus>;
  /** Models for the session agent picker. */
  models(): Promise<AgentModelsResponse>;
  /** Harness page data; refresh — also re-read caches the CLI keeps (models, latest version). */
  harness(refresh: boolean): Promise<HarnessInfo>;
  /** Set the agent's own default model (null — its built-in default). Present when harness().defaultModelEdit is set. */
  setDefaultModel?(model: string | null): void;
  /** Operation on a provider credential (accountMode: providers). Returns the CLI output. */
  accountOp?(provider: string, account: string, op: 'switch' | 'logout'): Promise<string>;
  /** Agent state the sandbox mounts writable (login, history, caches): absolute paths. */
  statePaths(): string[];
  /** Extra files the agent needs in a new project to pick up AGENTS.md (name → content). */
  contextFiles?: Record<string, string>;
  /**
   * The session folder moved (inbox → project): carry the agent's own transcript over.
   * true — the agent continues the same session; false — the hub sends a summary instead.
   */
  relocate?(session: Session, from: string, to: string): boolean;
  /** The agent's own MCP config (Harness → MCP); absent — the hub can't manage its servers. */
  mcp?: McpStore;
  /** Where the agent keeps skills, rules, subagents and commands (Harness → extensions). */
  extensions?(projectDir: string | null): ExtensionLayout;
}

/** A folder of extensions: skills (`<name>/SKILL.md`), subagents or commands (`*.md`). */
export interface ExtensionDir {
  dir: string;
  scope: ExtensionScope;
}

/** A rules file the agent reads; `ifExists` — listed only when present. */
export interface RulesSpec {
  label: string;
  path: string;
  scope: ExtensionScope;
  ifExists?: boolean;
}

export interface ExtensionLayout {
  skills: ExtensionDir[];
  rules: RulesSpec[];
  agents: ExtensionDir[];
  commands: ExtensionDir[];
  /** What the agent's config lists itself (plugins, plugin skills, agents in its JSON). */
  more?(): { skills?: ExtensionDir[]; agents?: SubagentInfo[]; plugins?: PluginInfo[] };
}
