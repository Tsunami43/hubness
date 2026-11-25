import { closeSync, existsSync, openSync, readdirSync, readFileSync, readSync, statSync } from 'node:fs';
import { join } from 'node:path';
import type { CommandInfo, HarnessExtensions, RulesFileContent, RulesFileInfo, SkillInfo, SubagentInfo } from '@hubness/shared';
import { agentOr404 } from './agents/registry.ts';
import { writeConfig } from './agents/mcp-config.ts';
import type { ExtensionDir, ExtensionLayout } from './agents/types.ts';
import { projects } from './db/index.ts';
import { HttpError } from './errors.ts';

/**
 * What an agent is extended with (Harness page): skills, rules files, subagents, commands
 * and plugins, read from the folders each agent module names. Read-only, except the rules
 * files the listing itself names, which can be opened and saved.
 */

const HEAD_BYTES = 64 * 1024;
const MAX_ENTRIES = 500;
const MAX_RULES = 1024 * 1024;

/** The first `max` bytes of a file as text ('' when unreadable). */
export function readHead(path: string, max = HEAD_BYTES): string {
  let fd: number | null = null;
  try {
    fd = openSync(path, 'r');
    const buf = Buffer.alloc(max);
    const n = readSync(fd, buf, 0, max, 0);
    return buf.subarray(0, n).toString('utf8');
  } catch {
    return '';
  } finally {
    if (fd !== null) closeSync(fd);
  }
}

const cut = (s: string, n: number) => Array.from(s).slice(0, n).join('');

function unquote(v: string): string {
  if (v.length >= 2 && v.startsWith('"') && v.endsWith('"')) {
    try {
      return JSON.parse(v);
    } catch {
      return v.slice(1, -1);
    }
  }
  if (v.length >= 2 && v.startsWith("'") && v.endsWith("'")) return v.slice(1, -1).replace(/''/g, "'");
  return v;
}

/**
 * YAML frontmatter, the small subset agents use: top-level `key: value` with plain or quoted
 * values, plain values continued on indented lines, and `>` / `|` block scalars. Nested
 * maps and lists are skipped. Never throws.
 */
export function parseFrontmatter(text: string): { fields: Record<string, string>; body: string } {
  const lines = text.replace(/^﻿/, '').split('\n').map((l) => l.replace(/\r$/, ''));
  if (lines[0]?.trim() !== '---') return { fields: {}, body: text };
  const end = lines.findIndex((l, i) => i > 0 && (l.trim() === '---' || l.trim() === '...'));
  if (end < 0) return { fields: {}, body: text };
  const fields: Record<string, string> = {};
  const indented = (l: string) => l.startsWith(' ') || l.startsWith('\t');
  let i = 1;
  while (i < end) {
    const m = lines[i]!.match(/^([A-Za-z0-9_-]+)\s*:(?:\s+(.*))?$/) ?? lines[i]!.match(/^([A-Za-z0-9_-]+)\s*:$/);
    i++;
    if (!m) continue;
    const key = m[1]!;
    let value = (m[2] ?? '').trim();
    // the following indented (or blank) lines
    const more: string[] = [];
    while (i < end && (indented(lines[i]!) || lines[i]!.trim() === '')) more.push(lines[i++]!);
    while (more.length && more[more.length - 1]!.trim() === '') more.pop();
    if (/^[>|][-+0-9]*$/.test(value)) {
      const parts = more.map((l) => l.trim());
      value = value.startsWith('>') ? parts.filter(Boolean).join(' ') : parts.join('\n');
    } else if (value.startsWith('"') || value.startsWith("'")) {
      value = unquote([value, ...more.map((l) => l.trim())].filter(Boolean).join(' '));
    } else if (value) {
      value = [value, ...more.map((l) => l.trim())].filter(Boolean).join(' ');
      const hash = value.indexOf(' #');
      if (hash >= 0) value = value.slice(0, hash);
    }
    fields[key] = value.trim();
  }
  return { fields, body: lines.slice(end + 1).join('\n') };
}

/** The first meaningful line of a markdown body (a command without a description). */
function firstLine(body: string): string {
  for (const l of body.split('\n')) {
    const s = l.replace(/^#+\s*/, '').trim();
    if (s) return cut(s, 200);
  }
  return '';
}

function names(dir: string): string[] {
  try {
    return readdirSync(dir)
      .filter((n) => !n.startsWith('.'))
      .sort()
      .slice(0, MAX_ENTRIES);
  } catch {
    return [];
  }
}

const isFile = (p: string) => {
  try {
    return statSync(p).isFile();
  } catch {
    return false;
  }
};
const isDir = (p: string) => {
  try {
    return statSync(p).isDirectory();
  } catch {
    return false;
  }
};

function skillsIn({ dir, scope }: ExtensionDir): SkillInfo[] {
  return names(dir).flatMap((n) => {
    const path = join(dir, n, 'SKILL.md');
    if (!isFile(path)) return [];
    const { fields } = parseFrontmatter(readHead(path));
    return [{ name: fields.name || n, description: cut(fields.description ?? '', 1000), path, scope }];
  });
}

function agentsIn({ dir, scope }: ExtensionDir): SubagentInfo[] {
  return names(dir).flatMap((n) => {
    const path = join(dir, n);
    if (!n.endsWith('.md') || !isFile(path)) return [];
    const { fields } = parseFrontmatter(readHead(path));
    return [{ name: fields.name || n.slice(0, -3), description: cut(fields.description ?? '', 1000), model: fields.model || null, path, scope }];
  });
}

function commandsIn({ dir, scope }: ExtensionDir): CommandInfo[] {
  const one = (path: string, name: string): CommandInfo => {
    const { fields, body } = parseFrontmatter(readHead(path));
    return { name, description: cut(fields.description || firstLine(body), 1000), path, scope };
  };
  return names(dir).flatMap((n) => {
    const path = join(dir, n);
    if (n.endsWith('.md') && isFile(path)) return [one(path, n.slice(0, -3))];
    // a sub-folder is a namespace: frontend/review.md → frontend:review
    if (isDir(path))
      return names(path)
        .filter((m) => m.endsWith('.md') && isFile(join(path, m)))
        .map((m) => one(join(path, m), `${n}:${m.slice(0, -3)}`));
    return [];
  });
}

function rulesOf(layout: ExtensionLayout): RulesFileInfo[] {
  return layout.rules.flatMap((r) => {
    const exists = isFile(r.path);
    if (r.ifExists && !exists) return [];
    const size = exists ? statSync(r.path).size : 0;
    const preview = exists ? cut(readHead(r.path, 4096), 300).trimEnd() || null : null;
    return [{ label: r.label, path: r.path, scope: r.scope, exists, size, preview }];
  });
}

/** The folder of `?project=<id>` (null without one). */
export function projectDirOf(id: string | undefined): string | null {
  if (!id) return null;
  const p = projects.get(id);
  if (!p) throw new HttpError(404, 'Project not found');
  return p.path;
}

function layoutOf(kind: string, projectDir: string | null): ExtensionLayout {
  const agent = agentOr404(kind);
  return agent.extensions?.(projectDir) ?? { skills: [], rules: [], agents: [], commands: [] };
}

export function extensionsOf(kind: string, projectDir: string | null): HarnessExtensions {
  const layout = layoutOf(kind, projectDir);
  const more = layout.more?.() ?? {};
  return {
    kind,
    skills: [...layout.skills, ...(more.skills ?? [])].flatMap(skillsIn),
    rules: rulesOf(layout),
    agents: [...layout.agents.flatMap(agentsIn), ...(more.agents ?? [])],
    commands: layout.commands.flatMap(commandsIn),
    plugins: more.plugins ?? [],
  };
}

/** Only a rules file the listing names can be opened or saved — never an arbitrary path. */
function listedRules(kind: string, projectDir: string | null, path: string | undefined): string {
  if (!path) throw new HttpError(400, 'path is required');
  const hit = rulesOf(layoutOf(kind, projectDir)).find((r) => r.path === path);
  if (!hit) throw new HttpError(403, 'Not a rules file of this agent');
  return hit.path;
}

export function readRules(kind: string, projectDir: string | null, path: string | undefined): RulesFileContent {
  const p = listedRules(kind, projectDir, path);
  if (!existsSync(p)) return { path: p, exists: false, content: '' };
  if (statSync(p).size > MAX_RULES) throw new HttpError(413, 'The file is too large to edit here');
  return { path: p, exists: true, content: readFileSync(p, 'utf8') };
}

export function saveRules(kind: string, projectDir: string | null, path: string | undefined, content: string): RulesFileContent {
  const p = listedRules(kind, projectDir, path);
  if (Buffer.byteLength(content) > MAX_RULES) throw new HttpError(413, 'content: at most 1 MB');
  writeConfig(p, content, false);
  return { path: p, exists: true, content };
}
