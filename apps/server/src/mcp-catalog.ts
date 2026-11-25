import { readFileSync } from 'node:fs';
import type { McpCatalogEntry, McpCatalogResponse, McpCatalogVar, McpInstallOption } from '@hubness/shared';
import { HttpError } from './errors.ts';

/**
 * The MCP catalog (Harness → MCP servers → Catalog): a search in the official MCP Registry
 * (registry.modelcontextprotocol.io, or HUBNESS_MCP_REGISTRY), proxied by the hub and
 * normalized into install options the add form fills in. An empty search shows a small
 * built-in list of popular servers (packages/shared/src/mcp-popular.json, registry format).
 */

const DEFAULT_REGISTRY = 'https://registry.modelcontextprotocol.io';
const PAGE = 30;
const TIMEOUT_MS = 10_000;
const TTL_MS = 5 * 60_000;
const MAX_CACHE = 50;

const registry = () => (process.env.HUBNESS_MCP_REGISTRY || DEFAULT_REGISTRY).replace(/\/+$/, '');

type Obj = Record<string, any>;
const isObj = (v: unknown): v is Obj => Boolean(v) && typeof v === 'object' && !Array.isArray(v);
const str = (v: unknown): string | null => (typeof v === 'string' && v !== '' ? v : null);
const list = (v: unknown): Obj[] => (Array.isArray(v) ? v.filter(isObj) : []);
const cut = (s: string, n: number) => Array.from(s).slice(0, n).join('');

const ENV_RE = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;
const HEADER_RE = /^[A-Za-z0-9_-]{1,128}$/;

function vars(v: unknown, re: RegExp): McpCatalogVar[] {
  return list(v)
    .filter((x) => typeof x.name === 'string' && re.test(x.name))
    .slice(0, 50)
    .map((x) => ({
      name: x.name,
      description: cut(str(x.description) ?? '', 500),
      required: x.isRequired === true,
      secret: x.isSecret === true,
      value: str(x.value) ?? str(x.default),
    }));
}

/** Registry arguments as argv; a required argument without a value becomes a `<hint>` placeholder. */
function argv(v: unknown): string[] {
  const out: string[] = [];
  for (const a of list(v)) {
    const value = str(a.value) ?? str(a.default);
    if (a.type === 'named') {
      const name = str(a.name);
      if (!name) continue;
      if (value !== null) out.push(name, value);
      else if (a.isRequired === true) {
        out.push(name);
        if (a.format !== 'boolean') out.push(`<${str(a.valueHint) ?? name.replace(/^-+/, '')}>`);
      }
    } else if (value !== null) out.push(value);
    else if (a.isRequired === true) out.push(`<${str(a.valueHint) ?? str(a.name) ?? 'value'}>`);
  }
  return out.slice(0, 100);
}

function packageOption(p: Obj): McpInstallOption | null {
  if (!isObj(p.transport) || p.transport.type !== 'stdio') return null;
  const id = str(p.identifier);
  if (!id) return null;
  const version = str(p.version);
  const env = vars(p.environmentVariables, ENV_RE);
  const runtime = argv(p.runtimeArguments);
  const rest = argv(p.packageArguments);
  const base = { transport: 'stdio' as const, url: null, env, headers: [] };
  switch (p.registryType) {
    case 'npm': {
      const command = str(p.runtimeHint) ?? 'npx';
      const pre = runtime.length ? runtime : command === 'npx' ? ['-y'] : [];
      return { kind: 'npm', command, args: [...pre, version ? `${id}@${version}` : id, ...rest], ...base };
    }
    case 'pypi': {
      const command = str(p.runtimeHint) ?? 'uvx';
      return { kind: 'pypi', command, args: [...runtime, version ? `${id}@${version}` : id, ...rest], ...base };
    }
    case 'oci': {
      const command = str(p.runtimeHint) ?? 'docker';
      const last = id.split('/').pop()!;
      const image = version && !last.includes(':') && !id.includes('@') ? `${id}:${version}` : id;
      const flags = runtime.length ? runtime : ['-i', '--rm'];
      return { kind: 'docker', command, args: ['run', ...flags, ...env.flatMap((e) => ['-e', e.name]), image, ...rest], ...base };
    }
    default:
      return null;
  }
}

function remoteOption(r: Obj): McpInstallOption | null {
  const transport = r.type === 'streamable-http' ? 'http' : r.type === 'sse' ? 'sse' : null;
  const url = str(r.url);
  if (!transport || !url || !/^https?:\/\//.test(url)) return null;
  return { kind: 'remote', transport, command: null, args: [], url, env: [], headers: vars(r.headers, HEADER_RE) };
}

const PREFIXES = ['mcp-server-', 'server-', 'mcp-'];
const SUFFIXES = ['-mcp-server', '-server', '-mcp'];

/** A short config name from a registry id: io.github.upstash/context7 → context7. */
export function suggestedName(id: string): string {
  let s = (id.split('/').pop() ?? '')
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, '-')
    .replace(/^-+|-+$/g, '');
  for (let changed = true; changed; ) {
    changed = false;
    const p = PREFIXES.find((x) => s.startsWith(x) && s.length > x.length);
    if (p) {
      s = s.slice(p.length);
      changed = true;
    }
    const x = SUFFIXES.find((y) => s.endsWith(y) && s.length > y.length);
    if (x) {
      s = s.slice(0, -x.length);
      changed = true;
    }
  }
  return s.slice(0, 64) || 'server';
}

/** One registry record (`{server, _meta}` or a bare server.json) as a catalog entry. */
export function catalogEntry(rec: unknown): McpCatalogEntry | null {
  const s = isObj(rec) && isObj(rec.server) ? rec.server : rec;
  if (!isObj(s)) return null;
  const id = str(s.name);
  if (!id) return null;
  const options = [...list(s.packages).map(packageOption), ...list(s.remotes).map(remoteOption)].filter((o): o is McpInstallOption => o !== null);
  return {
    id: cut(id, 200),
    title: cut(str(s.title) ?? id.split('/').pop()!, 200),
    description: cut(str(s.description) ?? '', 1000),
    version: str(s.version),
    repository: isObj(s.repository) ? str(s.repository.url) : null,
    website: str(s.websiteUrl),
    suggestedName: suggestedName(id),
    options: options.slice(0, 10),
  };
}

function entries(records: unknown[]): McpCatalogEntry[] {
  const seen = new Set<string>();
  const out: McpCatalogEntry[] = [];
  for (const r of records) {
    const e = catalogEntry(r);
    if (!e || seen.has(e.id)) continue;
    seen.add(e.id);
    out.push(e);
  }
  return out;
}

let popularCache: McpCatalogEntry[] | null = null;
function popular(): McpCatalogEntry[] {
  popularCache ??= entries(JSON.parse(readFileSync(new URL('../../../packages/shared/src/mcp-popular.json', import.meta.url), 'utf8')));
  return popularCache;
}

const cache = new Map<string, { at: number; value: McpCatalogResponse }>();

export async function mcpCatalog(q: string | undefined, cursor: string | undefined): Promise<McpCatalogResponse> {
  const query = (q ?? '').trim();
  const after = (cursor ?? '').trim();
  if (query.length > 200) throw new HttpError(400, 'q: at most 200 characters');
  if (after.length > 500) throw new HttpError(400, 'cursor: at most 500 characters');
  if (!query && !after) return { entries: popular(), nextCursor: null, source: 'popular' };

  const base = registry();
  const key = `${base}\n${query}\n${after}`;
  const hit = cache.get(key);
  if (hit && Date.now() - hit.at < TTL_MS) return hit.value;

  const params = new URLSearchParams({ limit: String(PAGE), version: 'latest' });
  if (query) params.set('search', query);
  if (after) params.set('cursor', after);
  let res: Response;
  try {
    res = await fetch(`${base}/v0/servers?${params}`, { headers: { accept: 'application/json', 'user-agent': 'hubness' }, signal: AbortSignal.timeout(TIMEOUT_MS) });
  } catch {
    throw new HttpError(502, 'The MCP Registry is unreachable — check the internet connection or try again later');
  }
  if (!res.ok) throw new HttpError(502, `The MCP Registry answered ${res.status} — try again later`);
  let body: unknown;
  try {
    body = await res.json();
  } catch {
    throw new HttpError(502, 'The MCP Registry sent an unreadable answer');
  }
  if (!isObj(body) || !Array.isArray(body.servers)) throw new HttpError(502, 'The MCP Registry sent an unreadable answer');
  const value: McpCatalogResponse = {
    entries: entries(body.servers),
    nextCursor: isObj(body.metadata) ? str(body.metadata.nextCursor) : null,
    source: 'registry',
  };
  if (cache.size >= MAX_CACHE) cache.delete(cache.keys().next().value!);
  cache.set(key, { at: Date.now(), value });
  return value;
}
