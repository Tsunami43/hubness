import type { AgentKind, McpAgentInfo, McpEntry, McpResponse, McpServerInfo, McpServerInput, McpTransport } from '@hubness/shared';
import type { McpDef } from './agents/mcp-config.ts';
import { AGENTS, agentOr404 } from './agents/registry.ts';
import type { AgentModule } from './agents/types.ts';
import { HttpError } from './errors.ts';

/**
 * MCP servers across the agents (Harness → MCP): the matrix "server × agent" read from each
 * agent's own config files, and adding / connecting / disconnecting servers in the agents'
 * user configs, so an agent sees them outside the hub too. Project configs are only shown.
 */

/** The hub's own server: attached to every hub session by the hub itself. */
export const BUILTIN_MCP = 'hub';

interface Found {
  agent: AgentModule;
  scope: 'user' | 'project';
  source: string;
  def: McpDef;
}

interface Scan {
  found: Found[];
  agents: McpAgentInfo[];
}

function scan(projectDir: string | null): Scan {
  const found: Found[] = [];
  const agents: McpAgentInfo[] = [];
  for (const agent of AGENTS) {
    const store = agent.mcp;
    if (!store) {
      agents.push({ kind: agent.kind, label: agent.label, supported: false, configPath: null, transports: [], error: 'The hub does not know where this agent keeps MCP servers' });
      continue;
    }
    const user = store.userFile();
    let error: string | null = null;
    try {
      for (const def of store.read(user)) found.push({ agent, scope: 'user', source: user, def });
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
    agents.push({ kind: agent.kind, label: agent.label, supported: true, configPath: user, transports: store.transports, error });
    if (!projectDir) continue;
    for (const { source, defs } of store.readProject(projectDir)) for (const def of defs) found.push({ agent, scope: 'project', source, def });
  }
  return { found, agents };
}

function builtinRow(): McpServerInfo {
  return {
    name: BUILTIN_MCP,
    transport: 'stdio',
    command: null,
    args: [],
    env: ['HUB_URL', 'HUB_SESSION_ID'],
    url: null,
    headers: [],
    builtIn: true,
    agents: Object.fromEntries(AGENTS.map((a) => [a.kind, 'builtin' as const])),
    entries: [],
  };
}

function matrix(s: Scan): McpResponse {
  const byName = new Map<string, Found[]>();
  for (const f of s.found) {
    if (f.def.name === BUILTIN_MCP) continue;
    byName.set(f.def.name, [...(byName.get(f.def.name) ?? []), f]);
  }
  const rows = [...byName.keys()]
    .sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))
    .map((name): McpServerInfo => {
      const all = byName.get(name)!;
      // the definition shown: the first user-scope one (agents in registry order)
      const main = (all.find((f) => f.scope === 'user') ?? all[0]!).def;
      const entries: McpEntry[] = [];
      for (const agent of AGENTS)
        for (const scope of ['user', 'project'] as const)
          for (const f of all.filter((x) => x.agent === agent && x.scope === scope))
            entries.push({ agent: agent.kind, scope, source: f.source, enabled: f.def.enabled });
      return {
        name,
        transport: main.transport,
        command: main.command,
        args: main.args,
        env: Object.keys(main.env),
        url: main.url,
        headers: Object.keys(main.headers),
        builtIn: false,
        agents: Object.fromEntries(AGENTS.map((a) => [a.kind, entries.find((e) => e.agent === a.kind)?.scope ?? null])),
        entries,
      };
    });
  return { servers: [builtinRow(), ...rows], agents: s.agents };
}

export const mcpMatrix = (projectDir: string | null): McpResponse => matrix(scan(projectDir));

// ---------- validation ----------

const NAME_RE = /^[A-Za-z0-9_-]{1,64}$/;
const ENV_RE = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;
const HEADER_RE = /^[A-Za-z0-9_-]{1,128}$/;

function checkName(name: string) {
  if (!NAME_RE.test(name)) throw new HttpError(400, 'name: letters, digits, - and _ only (at most 64)');
  if (name === BUILTIN_MCP) throw new HttpError(400, `“${BUILTIN_MCP}” is the hub's built-in server`);
}

function checkMap(m: Record<string, string>, re: RegExp, what: string) {
  const entries = Object.entries(m);
  if (entries.length > 100) throw new HttpError(400, `${what}: at most 100`);
  for (const [k, v] of entries) {
    if (!re.test(k)) throw new HttpError(400, `${what}: bad name “${k}”`);
    if (v.length > 4000 || /[\r\n\0]/.test(v)) throw new HttpError(400, `${what}: bad value of ${k}`);
  }
}

/** A definition from the request, checked; values are kept as given. */
function defOf(b: McpServerInput): McpDef {
  checkName(b.name);
  if (b.transport === 'stdio') {
    const command = (b.command ?? '').trim();
    if (!command || command.length > 1000 || /[\r\n\0]/.test(command)) throw new HttpError(400, 'command is required for a stdio server');
    const args = b.args ?? [];
    if (args.length > 100 || args.some((a) => a.length > 2000 || /\0/.test(a))) throw new HttpError(400, 'args: at most 100, 2000 characters each');
    const env = b.env ?? {};
    checkMap(env, ENV_RE, 'env');
    return { name: b.name, transport: 'stdio', command, args, env, url: null, headers: {}, enabled: true };
  }
  const url = (b.url ?? '').trim();
  if (!/^https?:\/\/[^\s]+$/.test(url) || url.length > 2000) throw new HttpError(400, 'url: an http(s) URL is required');
  const headers = b.headers ?? {};
  checkMap(headers, HEADER_RE, 'headers');
  return { name: b.name, transport: b.transport, command: null, args: [], env: {}, url, headers, enabled: true };
}

function supporting(kind: AgentKind, transport: McpTransport): AgentModule {
  const agent = agentOr404(kind);
  if (!agent.mcp) throw new HttpError(400, `${agent.label}: the hub can't manage its MCP servers`);
  if (!agent.mcp.transports.includes(transport)) throw new HttpError(400, `${agent.label} does not support ${transport.toUpperCase()} MCP servers`);
  return agent;
}

// ---------- changes (user configs only) ----------

/** Add a server to (or replace it in) each selected agent. All configs are checked before any is written. */
export function mcpAdd(b: McpServerInput, projectDir: string | null): McpResponse {
  const def = defOf(b);
  if (!b.agents.length) throw new HttpError(400, 'agents: pick at least one agent');
  const targets = [...new Set(b.agents)].map((k) => supporting(k, def.transport));
  for (const a of targets) a.mcp!.read(a.mcp!.userFile());
  for (const a of targets) a.mcp!.put(a.mcp!.userFile(), def);
  return mcpMatrix(projectDir);
}

/** Connect an existing server to one more agent: its definition is copied from another agent. */
export function mcpConnect(name: string, kind: AgentKind, projectDir: string | null): McpResponse {
  checkName(name);
  const s = scan(projectDir);
  const target = agentOr404(kind);
  if (s.found.some((f) => f.agent === target && f.scope === 'user' && f.def.name === name)) return matrix(s);
  const from = s.found.find((f) => f.def.name === name && f.scope === 'user') ?? s.found.find((f) => f.def.name === name);
  if (!from) throw new HttpError(404, 'MCP server not found');
  supporting(kind, from.def.transport);
  const file = target.mcp!.userFile();
  target.mcp!.read(file);
  target.mcp!.put(file, { ...from.def, enabled: true });
  return mcpMatrix(projectDir);
}

/** Remove a server from one agent's user config. */
export function mcpDisconnect(name: string, kind: AgentKind, projectDir: string | null): McpResponse {
  checkName(name);
  const agent = agentOr404(kind);
  if (!agent.mcp) throw new HttpError(400, `${agent.label}: the hub can't manage its MCP servers`);
  if (!agent.mcp.remove(agent.mcp.userFile(), name)) {
    const project = projectDir ? scan(projectDir).found.find((f) => f.agent === agent && f.def.name === name) : null;
    if (project) throw new HttpError(409, `It comes from the project config ${project.source} — edit that file`);
    throw new HttpError(404, 'This agent does not have the server');
  }
  return mcpMatrix(projectDir);
}

/** Remove a server from every agent's user config (unreadable configs are left alone). */
export function mcpDelete(name: string, projectDir: string | null): McpResponse {
  checkName(name);
  let removed = false;
  for (const a of AGENTS) {
    if (!a.mcp) continue;
    try {
      if (a.mcp.remove(a.mcp.userFile(), name)) removed = true;
    } catch {
      // unreadable: untouched
    }
  }
  if (!removed) throw new HttpError(404, 'MCP server not found');
  return mcpMatrix(projectDir);
}
