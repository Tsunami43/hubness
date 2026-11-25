import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, realpathSync, renameSync, statSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import { parse as parseToml } from 'smol-toml';
import type { McpTransport } from '@hubness/shared';
import { HttpError } from '../errors.ts';
import type { Json } from './kit.ts';

/**
 * MCP servers in the agents' own config files (JSON or TOML). The hub edits only the
 * server it is asked about: everything else in the file stays as it was. Writes are atomic
 * (a temp file renamed over the real path, keeping its mode), the first write keeps a
 * `<file>.hubness.bak`, and a file that doesn't parse is never touched.
 */

/** A server with its secrets (env / header values): never sent to clients as is. */
export interface McpDef {
  name: string;
  transport: McpTransport;
  command: string | null;
  args: string[];
  env: Record<string, string>;
  url: string | null;
  headers: Record<string, string>;
  enabled: boolean;
}

/** One agent's access to its MCP config files. */
export interface McpStore {
  /** The user config file the hub writes. */
  userFile(): string;
  /** Servers in the user file; [] when it is missing. Throws a 409 HttpError when it doesn't parse. */
  read(file: string): McpDef[];
  /** Servers a project adds (shown, never written); unreadable files are skipped. */
  readProject(dir: string): { source: string; defs: McpDef[] }[];
  /** Add or replace a server (user file only). */
  put(file: string, def: McpDef): void;
  /** Remove a server; false when it wasn't there. */
  remove(file: string, name: string): boolean;
  /** Transports the agent supports. */
  transports: McpTransport[];
}

// ---------- files ----------

const MAX_CONFIG = 16 * 1024 * 1024;

export const unreadable = (file: string, what: string) => new HttpError(409, `${file} is not valid ${what} — fix it or edit it by hand; the hub left it untouched`);

function readText(file: string): string | null {
  if (!existsSync(file)) return null;
  if (statSync(file).size > MAX_CONFIG) throw new HttpError(409, `${file} is too large to edit`);
  return readFileSync(file, 'utf8');
}

/**
 * Atomic write through the real path (a symlink stays a symlink), keeping the file mode;
 * with `backup`, the first write keeps a copy of the original as `<file>.hubness.bak`.
 */
export function writeConfig(file: string, text: string, backup = true) {
  const exists = existsSync(file);
  const real = exists ? realpathSync(file) : file;
  mkdirSync(dirname(real), { recursive: true });
  let mode = 0o644;
  if (exists) {
    mode = statSync(real).mode & 0o777;
    const bak = `${file}.hubness.bak`;
    if (backup && !existsSync(bak)) copyFileSync(real, bak);
  }
  const tmp = `${real}.hubness-${process.pid}.tmp`;
  writeFileSync(tmp, text, { mode });
  chmodSync(tmp, mode);
  renameSync(tmp, real);
}

// ---------- JSON configs ----------

const str = (v: unknown): string | null => (typeof v === 'string' ? v : null);
const strList = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []);
export function strMap(v: unknown): Record<string, string> {
  const out: Record<string, string> = {};
  if (v && typeof v === 'object' && !Array.isArray(v)) for (const [k, x] of Object.entries(v)) if (typeof x === 'string') out[k] = x;
  return out;
}
const isObj = (v: unknown): v is Json => Boolean(v) && typeof v === 'object' && !Array.isArray(v);

function readJsonFile(file: string): Json | null {
  const text = readText(file);
  if (text === null) return null;
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    throw unreadable(file, 'JSON');
  }
  if (!isObj(v)) throw unreadable(file, 'JSON');
  return v;
}

/** A project file and where its servers are (default: the same key as in the user file). */
export interface ProjectSource {
  file: string;
  pick?(cfg: Json): unknown;
}

interface JsonFormat {
  /** The key of the servers object. */
  key: string;
  /** A new user file's content. */
  init?: Json;
  toDef(name: string, v: Json): McpDef;
  fromDef(def: McpDef): Json;
  transports: McpTransport[];
}

export function jsonStore(f: JsonFormat & { userFile(): string; projectFiles(dir: string): ProjectSource[] }): McpStore {
  const defs = (servers: unknown) =>
    isObj(servers)
      ? Object.entries(servers)
          .filter(([, v]) => isObj(v))
          .map(([name, v]) => f.toDef(name, v))
      : [];
  return {
    userFile: f.userFile,
    transports: f.transports,
    read(file) {
      return defs(readJsonFile(file)?.[f.key]);
    },
    readProject(dir) {
      return f.projectFiles(dir).flatMap((p) => {
        try {
          const cfg = readJsonFile(p.file);
          return cfg ? [{ source: p.file, defs: defs(p.pick ? p.pick(cfg) : cfg[f.key]) }] : [];
        } catch {
          return [];
        }
      });
    },
    put(file, def) {
      const cfg = readJsonFile(file) ?? structuredClone(f.init ?? {});
      if (!isObj(cfg[f.key])) cfg[f.key] = {};
      cfg[f.key][def.name] = f.fromDef(def);
      writeConfig(file, JSON.stringify(cfg, null, 2) + '\n');
    },
    remove(file, name) {
      const cfg = readJsonFile(file);
      if (!cfg || !isObj(cfg[f.key]) || !(name in cfg[f.key])) return false;
      delete cfg[f.key][name];
      writeConfig(file, JSON.stringify(cfg, null, 2) + '\n');
      return true;
    },
  };
}

/** Claude Code: `mcpServers` ({type, command, args, env} / {type, url, headers}). */
export const claudeFormat: JsonFormat = {
  key: 'mcpServers',
  transports: ['stdio', 'http', 'sse'],
  toDef(name, v) {
    const url = str(v.url);
    const type = str(v.type);
    const transport: McpTransport = type === 'sse' ? 'sse' : type === 'http' ? 'http' : url && !v.command ? 'http' : 'stdio';
    return { name, transport, command: str(v.command), args: strList(v.args), env: strMap(v.env), url, headers: strMap(v.headers), enabled: true };
  },
  fromDef(d) {
    if (d.transport === 'stdio') return { type: 'stdio', command: d.command, args: d.args, env: d.env };
    return { type: d.transport, url: d.url, ...(Object.keys(d.headers).length ? { headers: d.headers } : {}) };
  },
};

/** OpenCode: `mcp` ({type: local, command: [...], environment} / {type: remote, url, headers}). */
export const opencodeFormat: JsonFormat = {
  key: 'mcp',
  init: { $schema: 'https://opencode.ai/config.json' },
  transports: ['stdio', 'http', 'sse'],
  toDef(name, v) {
    const enabled = v.enabled !== false;
    if (v.type === 'remote' || (str(v.url) && v.type !== 'local'))
      return { name, transport: 'http', command: null, args: [], env: {}, url: str(v.url), headers: strMap(v.headers), enabled };
    const cmd = strList(v.command);
    return { name, transport: 'stdio', command: cmd[0] ?? null, args: cmd.slice(1), env: strMap(v.environment), url: null, headers: {}, enabled };
  },
  fromDef(d) {
    if (d.transport === 'stdio')
      return { type: 'local', command: [d.command, ...d.args], ...(Object.keys(d.env).length ? { environment: d.env } : {}), enabled: true };
    // OpenCode finds out itself whether a remote server speaks streamable HTTP or SSE
    return { type: 'remote', url: d.url, ...(Object.keys(d.headers).length ? { headers: d.headers } : {}), enabled: true };
  },
};

// ---------- TOML configs (Codex, Grok): `[mcp_servers.<name>]` tables ----------

interface TomlFormat {
  /** The key of the header map: Codex `http_headers`, Grok `headers`. */
  headersKey: string;
  /** With SSE (Grok), `type = "sse"` marks an SSE server; Codex has none. */
  transports: McpTransport[];
  userFile(): string;
  projectFiles(dir: string): string[];
}

function readTomlFile(file: string): { text: string; doc: Json } | null {
  const text = readText(file);
  if (text === null) return null;
  try {
    return { text, doc: parseToml(text) as Json };
  } catch {
    throw unreadable(file, 'TOML');
  }
}

const tomlStr = (s: string) => JSON.stringify(s);
const tomlInline = (m: Record<string, string>) => `{ ${Object.entries(m).map(([k, v]) => `${tomlKey(k)} = ${tomlStr(v)}`).join(', ')} }`;
const tomlKey = (k: string) => (/^[A-Za-z0-9_-]+$/.test(k) ? k : tomlStr(k));

/** The dotted key of a table header line (`[a."b.c"]` → ['a', 'b.c']); null — not a table header. */
export function tomlHeader(line: string): string[] | null {
  const m = line.match(/^\s*\[(?!\[)(.*)\]\s*(#.*)?$/);
  if (!m) return null;
  const parts: string[] = [];
  let s = m[1]!.trim();
  while (s) {
    let key: string;
    if (s[0] === '"' || s[0] === "'") {
      const q = s[0];
      let i = 1;
      while (i < s.length && s[i] !== q) i += q === '"' && s[i] === '\\' ? 2 : 1;
      if (i >= s.length) return null;
      const raw = s.slice(0, i + 1);
      try {
        key = q === '"' ? JSON.parse(raw) : raw.slice(1, -1);
      } catch {
        return null;
      }
      s = s.slice(i + 1).trim();
    } else {
      const m2 = s.match(/^[A-Za-z0-9_-]+/);
      if (!m2) return null;
      key = m2[0];
      s = s.slice(key.length).trim();
    }
    parts.push(key);
    if (!s) break;
    if (s[0] !== '.') return null;
    s = s.slice(1).trim();
  }
  return parts.length ? parts : null;
}

const isHeader = (line: string) => /^\s*\[/.test(line) && (tomlHeader(line) !== null || /^\s*\[\[/.test(line));

/** Line ranges [start, end) of the tables of server `name` (its own and sub-tables). */
function serverBlocks(lines: string[], name: string): [number, number][] {
  const out: [number, number][] = [];
  for (let i = 0; i < lines.length; i++) {
    const h = tomlHeader(lines[i]!);
    if (!h || h[0] !== 'mcp_servers' || h[1] !== name) continue;
    let end = i + 1;
    while (end < lines.length && !isHeader(lines[end]!)) end++;
    // comments and blank lines right before the next table belong to it
    while (end > i + 1 && /^\s*(#.*)?$/.test(lines[end - 1]!)) end--;
    out.push([i, end]);
  }
  return out;
}

function removeBlocks(lines: string[], name: string): string[] {
  const blocks = serverBlocks(lines, name).map(([a, b]): [number, number] => {
    // the blank line that separated the block goes with it
    const blank = (i: number) => i >= lines.length || lines[i]!.trim() === '';
    return a > 0 && blank(a - 1) && blank(b) ? [a - 1, b] : [a, b];
  });
  return lines.filter((_, i) => !blocks.some(([a, b]) => i >= a && i < b));
}

/** Sorted-key JSON: the comparison of two parsed documents. */
function canon(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canon).join(',')}]`;
  // plain objects; smol-toml parses tables into null-prototype objects
  if (v && typeof v === 'object' && [Object.prototype, null].includes(Object.getPrototypeOf(v)))
    return `{${Object.keys(v)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canon((v as Json)[k])}`)
      .join(',')}}`;
  return JSON.stringify(v);
}

function withoutServer(doc: Json, name: string): Json {
  const copy = { ...doc };
  if (isObj(copy.mcp_servers)) {
    const servers = { ...copy.mcp_servers };
    delete servers[name];
    if (Object.keys(servers).length) copy.mcp_servers = servers;
    else delete copy.mcp_servers;
  }
  return copy;
}

export function tomlStore(f: TomlFormat): McpStore {
  const toDef = (name: string, v: Json, disabled: string[]): McpDef => {
    const url = str(v.url);
    const transport: McpTransport = v.type === 'sse' ? 'sse' : url && !v.command ? 'http' : 'stdio';
    return {
      name,
      transport,
      command: str(v.command),
      args: strList(v.args),
      env: strMap(v.env),
      url,
      headers: strMap(v[f.headersKey]),
      enabled: v.enabled !== false && !disabled.includes(name),
    };
  };
  const render = (d: McpDef): string[] => {
    const out = [`[mcp_servers.${tomlKey(d.name)}]`];
    if (d.transport === 'stdio') {
      out.push(`command = ${tomlStr(d.command ?? '')}`);
      out.push(`args = [${d.args.map(tomlStr).join(', ')}]`);
      if (Object.keys(d.env).length) out.push(`env = ${tomlInline(d.env)}`);
    } else {
      if (d.transport === 'sse') out.push('type = "sse"');
      out.push(`url = ${tomlStr(d.url ?? '')}`);
      if (Object.keys(d.headers).length) out.push(`${f.headersKey} = ${tomlInline(d.headers)}`);
    }
    return out;
  };
  /** Write the edited text only when it parses to exactly the expected document. */
  const commit = (file: string, text: string, expected: Json) => {
    let got: Json;
    try {
      got = parseToml(text) as Json;
    } catch {
      got = { broken: true };
    }
    if (canon(withoutServer(got, '')) !== canon(withoutServer(expected, '')))
      throw new HttpError(409, `${file}: the hub can't change this server without touching the rest of the file — edit it by hand`);
    writeConfig(file, text);
  };
  const read = (file: string): McpDef[] => {
    const t = readTomlFile(file);
    if (!t || !isObj(t.doc.mcp_servers)) return [];
    const disabled = strList(t.doc.disabled_mcp_servers);
    return Object.entries(t.doc.mcp_servers as Json)
      .filter(([, v]) => isObj(v))
      .map(([name, v]) => toDef(name, v, disabled));
  };
  return {
    userFile: f.userFile,
    transports: f.transports,
    read,
    readProject(dir) {
      return f.projectFiles(dir).flatMap((file) => {
        if (file === f.userFile() || !existsSync(file)) return [];
        try {
          return [{ source: file, defs: read(file) }];
        } catch {
          return [];
        }
      });
    },
    put(file, def) {
      const t = readTomlFile(file) ?? { text: '', doc: {} };
      const lines = removeBlocks(t.text.split('\n'), def.name);
      // after the last server table, or at the end
      let at = -1;
      for (let i = 0; i < lines.length; i++) {
        const h = tomlHeader(lines[i]!);
        if (h?.[0] === 'mcp_servers') at = i;
      }
      let insertAt = lines.length;
      if (at >= 0) {
        insertAt = at + 1;
        while (insertAt < lines.length && !isHeader(lines[insertAt]!)) insertAt++;
        while (insertAt > at + 1 && /^\s*(#.*)?$/.test(lines[insertAt - 1]!)) insertAt--;
      } else {
        while (insertAt > 0 && lines[insertAt - 1]!.trim() === '') insertAt--;
      }
      const block = render(def);
      const before = lines.slice(0, insertAt);
      const after = lines.slice(insertAt);
      const sep = before.length && before[before.length - 1]!.trim() !== '' ? [''] : [];
      const tail = after.length && after[0]!.trim() !== '' ? [''] : [];
      let text = [...before, ...sep, ...block, ...tail, ...after].join('\n');
      if (!text.endsWith('\n')) text += '\n';
      const expected = withoutServer(t.doc, def.name);
      const own = (parseToml(block.join('\n')) as Json).mcp_servers[def.name];
      expected.mcp_servers = { ...(isObj(expected.mcp_servers) ? expected.mcp_servers : {}), [def.name]: own };
      commit(file, text, expected);
    },
    remove(file, name) {
      const t = readTomlFile(file);
      if (!t || !isObj(t.doc.mcp_servers) || !(name in t.doc.mcp_servers)) return false;
      const text = removeBlocks(t.text.split('\n'), name).join('\n');
      commit(file, text, withoutServer(t.doc, name));
      return true;
    },
  };
}
