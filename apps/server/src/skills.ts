import { execFile } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { copyFileSync, cpSync, existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join, sep } from 'node:path';
import type { AgentKind, SkillContent, SkillCreateInput, SkillEntry, SkillFile, SkillImportFound, SkillImportInput, SkillImportPreview, SkillRow, SkillScope, SkillsResponse } from '@hubness/shared';
import { AGENTS, agentOr404 } from './agents/registry.ts';
import type { AgentModule, ExtensionDir } from './agents/types.ts';
import { childEnv, config } from './config.ts';
import { HttpError } from './errors.ts';
import { parseFrontmatter, readHead } from './extensions.ts';

/**
 * Skills across the agents (Harness → Skills): the matrix "skill × agent" read from each
 * agent's skills folders, and copying, creating, importing (from a git repository) and
 * removing skills in the agents' user folders. A skill is a folder with SKILL.md, keyed by
 * the folder name; the format is the same for every agent, so a folder is copied as is.
 * Removed skills are moved into the hub trash (<data>/trash/skills), never deleted.
 * Project and plugin skills are only shown.
 */

const NAME_RE = /^[A-Za-z0-9_-]{1,64}$/;
const MAX_ENTRIES = 500;
const MAX_FILES = 500;
const MAX_BYTES = 50 * 1024 * 1024;
const MAX_DEPTH = 8;
const MAX_SKILL_MD = 1024 * 1024;
const IMPORT_TTL_MS = 15 * 60_000;
const CLONE_TIMEOUT_MS = 120_000;
const SCOPES: SkillScope[] = ['user', 'project', 'plugin'];

const isDir = (p: string) => {
  try {
    return statSync(p).isDirectory();
  } catch {
    return false;
  }
};
const isFile = (p: string) => {
  try {
    return statSync(p).isFile();
  } catch {
    return false;
  }
};
const cut = (s: string, n: number) => Array.from(s).slice(0, n).join('');
const byBytes = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);

function names(dir: string): string[] {
  try {
    return readdirSync(dir)
      .filter((n) => !n.startsWith('.'))
      .sort(byBytes)
      .slice(0, MAX_ENTRIES);
  } catch {
    return [];
  }
}

export function checkSkillName(name: string) {
  if (!NAME_RE.test(name)) throw new HttpError(400, 'name: letters, digits, - and _ only (at most 64)');
}

// ---------- where the agents keep skills ----------

function skillDirs(agent: AgentModule, projectDir: string | null): ExtensionDir[] {
  const layout = agent.extensions?.(projectDir);
  if (!layout) return [];
  return [...layout.skills, ...(layout.more?.().skills ?? [])];
}

/** The user skills folder the hub writes: the last one that exists, else the last one named. */
function writeDir(agent: AgentModule): string | null {
  const dirs = skillDirs(agent, null)
    .filter((d) => d.scope === 'user')
    .map((d) => d.dir);
  if (!dirs.length) return null;
  const existing = dirs.filter(isDir);
  return existing.length ? existing[existing.length - 1]! : dirs[dirs.length - 1]!;
}

interface Found {
  agent: AgentModule;
  scope: SkillScope;
  /** The skill folder. */
  dir: string;
  name: string;
}

function scan(projectDir: string | null): Found[] {
  const out: Found[] = [];
  for (const agent of AGENTS)
    for (const d of skillDirs(agent, projectDir))
      for (const n of names(d.dir)) if (isFile(join(d.dir, n, 'SKILL.md'))) out.push({ agent, scope: d.scope, dir: join(d.dir, n), name: n });
  return out;
}

const rank = (s: SkillScope) => SCOPES.indexOf(s);
/** The best copy: user over project over plugin, agents in registry order. */
const best = (all: Found[]) => [...all].sort((a, b) => rank(a.scope) - rank(b.scope))[0];

function description(dir: string): string {
  return cut(parseFrontmatter(readHead(join(dir, 'SKILL.md'))).fields.description ?? '', 1000);
}

function matrix(found: Found[]): SkillsResponse {
  const byName = new Map<string, Found[]>();
  for (const f of found) byName.set(f.name, [...(byName.get(f.name) ?? []), f]);
  const skills = [...byName.keys()].sort(byBytes).map((name): SkillRow => {
    const all = byName.get(name)!;
    const entries: SkillEntry[] = all.map((f) => ({ agent: f.agent.kind, scope: f.scope, path: f.dir }));
    return {
      name,
      description: description(best(all)!.dir),
      agents: Object.fromEntries(AGENTS.map((a) => [a.kind, best(all.filter((f) => f.agent === a))?.scope ?? null])),
      entries,
    };
  });
  const agents = AGENTS.map((a) => {
    const dir = writeDir(a);
    return { kind: a.kind, label: a.label, supported: dir !== null, dir };
  });
  return { skills, agents };
}

export const skillsMatrix = (projectDir: string | null): SkillsResponse => matrix(scan(projectDir));

// ---------- skill folders ----------

interface TreeFile {
  rel: string;
  abs: string;
  size: number;
}

/**
 * The files of a skill folder (sorted, `.git` skipped). Symlinks are followed only to files
 * inside the folder itself; anything else is left out. Over the limits: a 413, or with
 * `lenient` the listing is cut.
 */
function tree(root: string, lenient = false): TreeFile[] {
  const real = realpathSync(root);
  const out: TreeFile[] = [];
  let bytes = 0;
  const walk = (dir: string, rel: string, depth: number) => {
    let items: string[];
    try {
      items = readdirSync(dir).sort(byBytes);
    } catch {
      return;
    }
    for (const n of items) {
      if (n === '.git') continue;
      const abs = join(dir, n);
      const r = rel ? `${rel}/${n}` : n;
      const st = lstatSync(abs);
      let file = abs;
      if (st.isSymbolicLink()) {
        let target: string;
        try {
          target = realpathSync(abs);
        } catch {
          continue;
        }
        if (!target.startsWith(real + sep) || !isFile(target)) continue;
        file = target;
      } else if (st.isDirectory()) {
        if (depth < MAX_DEPTH) walk(abs, r, depth + 1);
        continue;
      } else if (!st.isFile()) continue;
      const size = statSync(file).size;
      if (out.length >= MAX_FILES || bytes + size > MAX_BYTES) {
        if (lenient) return;
        throw new HttpError(413, `The skill is too large (at most ${MAX_FILES} files, 50 MB)`);
      }
      bytes += size;
      out.push({ rel: r, abs: file, size });
    }
  };
  walk(real, '', 0);
  return out;
}

function sameTree(a: string, b: string): boolean {
  try {
    const x = tree(a);
    const y = tree(b);
    return x.length === y.length && x.every((f, i) => f.rel === y[i]!.rel && f.size === y[i]!.size && readFileSync(f.abs).equals(readFileSync(y[i]!.abs)));
  } catch {
    return false;
  }
}

/** Write `files` as the folder `<dir>/<name>`: into a temp folder first, then renamed into place. */
function writeSkill(dir: string, name: string, files: { rel: string; abs?: string; text?: string }[]) {
  mkdirSync(dir, { recursive: true });
  const tmp = join(dir, `.hubness-${name}-${process.pid}-${Date.now()}`);
  try {
    for (const f of files) {
      const dest = join(tmp, f.rel);
      mkdirSync(join(dest, '..'), { recursive: true });
      if (f.abs !== undefined) copyFileSync(f.abs, dest);
      else writeFileSync(dest, f.text ?? '');
    }
    mkdirSync(tmp, { recursive: true });
    renameSync(tmp, join(dir, name));
  } catch (e) {
    rmSync(tmp, { recursive: true, force: true });
    throw e;
  }
}

/** Move a skill folder (or a symlink to one) into the hub trash. */
function toTrash(path: string, agent: AgentKind, name: string): string {
  const dir = join(config.dataDir, 'trash', 'skills');
  mkdirSync(dir, { recursive: true });
  let target = join(dir, `${Date.now()}-${agent}-${name}`);
  for (let i = 2; existsSync(target); i++) target = join(dir, `${Date.now()}-${agent}-${name}-${i}`);
  try {
    renameSync(path, target);
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code !== 'EXDEV') throw e;
    cpSync(path, target, { recursive: true, verbatimSymlinks: true });
    rmSync(path, { recursive: true, force: true });
  }
  return target;
}

function writable(kind: AgentKind): { agent: AgentModule; dir: string } {
  const agent = agentOr404(kind);
  const dir = writeDir(agent);
  if (!dir) throw new HttpError(400, `${agent.label}: the hub doesn't know where this agent keeps skills`);
  return { agent, dir };
}

function targetsOf(kinds: AgentKind[]): { agent: AgentModule; dir: string }[] {
  if (!kinds.length) throw new HttpError(400, 'agents: pick at least one agent');
  return [...new Set(kinds)].map(writable);
}

/** A user copy of the skill in this agent (any of its user folders). */
function userCopy(found: Found[], agent: AgentModule, name: string): Found | undefined {
  return found.find((f) => f.agent === agent && f.scope === 'user' && f.name === name);
}

// ---------- reading ----------

export function skillContent(name: string, kind: string | undefined, projectDir: string | null): SkillContent {
  checkSkillName(name);
  const all = scan(projectDir).filter((f) => f.name === name);
  const pick = kind ? best(all.filter((f) => f.agent === agentOr404(kind))) : best(all);
  if (!pick) throw new HttpError(404, 'Skill not found');
  const md = join(pick.dir, 'SKILL.md');
  if (statSync(md).size > MAX_SKILL_MD) throw new HttpError(413, 'SKILL.md is too large to show');
  const files: SkillFile[] = tree(pick.dir, true).map((f) => ({ path: f.rel, size: f.size }));
  return { name, agent: pick.agent.kind, scope: pick.scope, path: pick.dir, content: readFileSync(md, 'utf8'), files };
}

// ---------- changes (user folders only) ----------

/** Copy a skill to one more agent, from the agent that has it (user copies first). */
export function skillConnect(name: string, kind: string, projectDir: string | null): SkillsResponse {
  checkSkillName(name);
  const found = scan(projectDir);
  const { agent, dir } = writable(kind);
  if (userCopy(found, agent, name)) return matrix(found);
  const from = best(found.filter((f) => f.name === name));
  if (!from) throw new HttpError(404, 'Skill not found');
  if (existsSync(join(dir, name))) throw new HttpError(409, `${join(dir, name)} exists but is not a skill — move it away first`);
  writeSkill(dir, name, tree(from.dir));
  return skillsMatrix(projectDir);
}

/** Move one agent's user copy into the hub trash. */
export function skillDisconnect(name: string, kind: string, projectDir: string | null): SkillsResponse {
  checkSkillName(name);
  const found = scan(projectDir);
  const agent = agentOr404(kind);
  const own = userCopy(found, agent, name);
  if (!own) {
    const other = found.find((f) => f.agent === agent && f.name === name);
    if (other) throw new HttpError(409, `It comes from ${other.scope === 'plugin' ? 'a plugin' : 'the project'} (${other.dir}) — the hub doesn't change it`);
    throw new HttpError(404, 'This agent does not have the skill');
  }
  for (const f of found.filter((x) => x.agent === agent && x.scope === 'user' && x.name === name)) toTrash(f.dir, agent.kind, name);
  return skillsMatrix(projectDir);
}

/** Move every user copy into the hub trash. */
export function skillDelete(name: string, projectDir: string | null): SkillsResponse {
  checkSkillName(name);
  const own = scan(projectDir).filter((f) => f.scope === 'user' && f.name === name);
  if (!own.length) throw new HttpError(404, 'Skill not found');
  for (const f of own) toTrash(f.dir, f.agent.kind, name);
  return skillsMatrix(projectDir);
}

/**
 * Put a skill folder into each target; an existing different skill of the same name is a 409
 * unless `replace` (it then goes to the trash). All targets are checked before any is written.
 */
function install(name: string, files: { rel: string; abs?: string; text?: string }[], source: string | null, targets: { agent: AgentModule; dir: string }[], replace: boolean) {
  const plan: { agent: AgentModule; dir: string; existing: string | null }[] = [];
  for (const t of targets) {
    const path = join(t.dir, name);
    if (!existsSync(path)) {
      plan.push({ ...t, existing: null });
      continue;
    }
    if (source && sameTree(source, path)) continue;
    if (!source && isFile(join(path, 'SKILL.md')) && files.length === 1) {
      const text = readFileSync(join(path, 'SKILL.md'), 'utf8');
      if (text === files[0]!.text && tree(path, true).length === 1) continue;
    }
    if (!replace) throw new HttpError(409, `${t.agent.label} already has a different skill “${name}” — pass replace to swap it (the old one goes to the hub trash)`);
    plan.push({ ...t, existing: path });
  }
  for (const p of plan) {
    if (p.existing) toTrash(p.existing, p.agent.kind, name);
    writeSkill(p.dir, name, files);
  }
}

export function skillMarkdown(name: string, description: string, body: string): string {
  return `---\nname: ${name}\ndescription: ${JSON.stringify(description)}\n---\n\n${body}${body.endsWith('\n') ? '' : '\n'}`;
}

export function skillCreate(b: SkillCreateInput, projectDir: string | null): SkillsResponse {
  checkSkillName(b.name);
  const description = b.description.trim();
  if (!description) throw new HttpError(400, 'description: say when the agent should use the skill');
  if (Array.from(description).length > 1024) throw new HttpError(400, 'description: at most 1024 characters');
  if (Buffer.byteLength(b.body) > MAX_SKILL_MD) throw new HttpError(413, 'body: at most 1 MB');
  const targets = targetsOf(b.agents);
  install(b.name, [{ rel: 'SKILL.md', text: skillMarkdown(b.name, description, b.body) }], null, targets, b.replace === true);
  return skillsMatrix(projectDir);
}

// ---------- import from a git repository ----------

const importsDir = () => join(config.dataDir, 'tmp', 'skill-imports');
const ID_RE = /^[0-9a-f-]{36}$/;

/** owner/repo → https://github.com/owner/repo; https URLs as given (a GitHub page URL → its repo). */
export function importUrl(raw: string): string {
  const s = raw.trim();
  if (/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(s)) return `https://github.com/${s}`;
  if (s.length > 2000 || /[\s\0]/.test(s)) throw new HttpError(400, 'url: an https:// git URL or owner/repo');
  const gh = s.match(/^https:\/\/github\.com\/([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+)\/.+$/);
  if (gh) return `https://github.com/${gh[1]}/${gh[2]}`;
  if (/^https:\/\/[^/]+\/\S+$/.test(s)) return s;
  if (process.env.HUBNESS_ALLOW_FILE_GIT === '1' && /^file:\/\/\/\S+$/.test(s)) return s;
  throw new HttpError(400, 'url: an https:// git URL or owner/repo');
}

/** Remove clones older than the TTL. */
function sweepImports() {
  for (const n of names(importsDir())) {
    const p = join(importsDir(), n);
    try {
      if (Date.now() - statSync(p).mtimeMs > IMPORT_TTL_MS) rmSync(p, { recursive: true, force: true });
    } catch {
      // gone already
    }
  }
}

function clone(url: string, dest: string): Promise<void> {
  const file = url.startsWith('file://');
  const args = ['-c', 'core.symlinks=false', '-c', 'protocol.allow=never', '-c', `protocol.${file ? 'file' : 'https'}.allow=always`, 'clone', '--depth', '1', '--quiet', '--no-tags', '--', url, dest];
  return new Promise((ok, fail) => {
    execFile('git', args, { env: { ...childEnv(), GIT_TERMINAL_PROMPT: '0' }, timeout: CLONE_TIMEOUT_MS, maxBuffer: 1024 * 1024 }, (err, _out, stderr) => {
      if (!err) return ok();
      const why = String(stderr).trim().split('\n').filter(Boolean).pop() ?? (err.killed ? 'timed out' : err.message);
      fail(new HttpError(422, `git clone failed: ${cut(why, 300)}`));
    });
  });
}

/** Skill folders in a clone (depth-first, sorted; a skill folder is not searched further). */
function findSkills(root: string): (SkillImportFound & { abs: string })[] {
  const out: (SkillImportFound & { abs: string })[] = [];
  const seen = new Set<string>();
  const walk = (dir: string, rel: string, depth: number) => {
    if (out.length >= 200) return;
    const md = join(dir, 'SKILL.md');
    if (existsSync(md) && lstatSync(md).isFile()) {
      const { fields } = parseFrontmatter(readHead(md));
      const name = rel ? rel.split('/').pop()! : (fields.name ?? '');
      if (NAME_RE.test(name) && !seen.has(name)) {
        seen.add(name);
        out.push({ name, description: cut(fields.description ?? '', 1000), path: rel || '.', abs: dir });
      }
      return;
    }
    if (depth >= 6) return;
    let items: string[];
    try {
      items = readdirSync(dir).sort(byBytes);
    } catch {
      return;
    }
    for (const n of items) {
      if (n === '.git' || n === 'node_modules') continue;
      const p = join(dir, n);
      if (lstatSync(p).isDirectory()) walk(p, rel ? `${rel}/${n}` : n, depth + 1);
    }
  };
  walk(root, '', 0);
  return out;
}

export async function skillImportPreview(rawUrl: string): Promise<SkillImportPreview> {
  const url = importUrl(rawUrl);
  sweepImports();
  const id = randomUUID();
  const dir = join(importsDir(), id);
  mkdirSync(dir, { recursive: true });
  try {
    await clone(url, join(dir, 'repo'));
  } catch (e) {
    rmSync(dir, { recursive: true, force: true });
    throw e;
  }
  const skills = findSkills(join(dir, 'repo')).map(({ name, description, path }) => ({ name, description, path }));
  if (!skills.length) {
    rmSync(dir, { recursive: true, force: true });
    throw new HttpError(422, 'No skills found in the repository (folders with a SKILL.md)');
  }
  return { id, url, skills };
}

export function skillImport(b: SkillImportInput, projectDir: string | null): SkillsResponse {
  if (!ID_RE.test(b.id)) throw new HttpError(400, 'id: bad import id');
  sweepImports();
  const dir = join(importsDir(), b.id);
  if (!isDir(join(dir, 'repo'))) throw new HttpError(404, 'The import has expired — look the repository up again');
  if (!b.skills.length) throw new HttpError(400, 'skills: pick at least one skill');
  const found = findSkills(join(dir, 'repo'));
  const chosen = [...new Set(b.skills)].map((n) => {
    const hit = found.find((f) => f.name === n);
    if (!hit) throw new HttpError(404, `No skill “${n}” in the repository`);
    return hit;
  });
  const targets = targetsOf(b.agents);
  const plans = chosen.map((s) => ({ s, files: tree(s.abs) }));
  // check every skill before writing any: a conflict leaves everything as it was
  if (b.replace !== true)
    for (const { s } of plans)
      for (const t of targets) {
        const path = join(t.dir, s.name);
        if (existsSync(path) && !sameTree(s.abs, path))
          throw new HttpError(409, `${t.agent.label} already has a different skill “${s.name}” — pass replace to swap it (the old one goes to the hub trash)`);
      }
  for (const { s, files } of plans) install(s.name, files, s.abs, targets, b.replace === true);
  rmSync(dir, { recursive: true, force: true });
  return skillsMatrix(projectDir);
}
