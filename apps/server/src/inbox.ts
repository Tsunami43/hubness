import { cpSync, existsSync, mkdirSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { basename, extname, join } from 'node:path';
import type { Project, ProjectSuggestion, Session } from '@hubness/shared';
import { findAgent } from './agents/registry.ts';
import { config } from './config.ts';
import { events, projects, sessions } from './db/index.ts';
import { HttpError } from './errors.ts';
import { createManifest } from './manifest.ts';
import { projectsDir } from './catalog.ts';
import { sh } from './system/monitor.ts';

/**
 * Inbox → Project. New chats live in ~/hub/inbox/<date>-<topic>/; when
 * code appears in a chat, the hub offers to move it to ~/hub/projects/<name>
 * and continue the same session in the project.
 */

// Cyrillic → Latin, so folder names stay ASCII: shells, globs, Docker Compose project names
const TRANSLIT: Record<string, string> = Object.fromEntries(
  [...'абвгдеёжзийклмнопрстуфхцчшщъыьэюяіїєґ'].map((c, i) =>
    [c, ['a', 'b', 'v', 'g', 'd', 'e', 'e', 'zh', 'z', 'i', 'y', 'k', 'l', 'm', 'n', 'o', 'p', 'r', 's', 't', 'u', 'f', 'kh', 'ts', 'ch', 'sh', 'shch', '', 'y', '', 'e', 'yu', 'ya', 'i', 'yi', 'ye', 'g'][i]!],
  ),
);

export const slug = (s: string) =>
  [...s.toLowerCase()]
    .map((c) => TRANSLIT[c] ?? c)
    .join('')
    .normalize('NFKD')
    .replace(/\p{M}/gu, '')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    // at most 40 chars, cut at a word boundary (unless the first word alone is longer)
    .replace(/^(.{1,40})(?:-.*)?$|^(.{40}).*$/, '$1$2') || 'chat';

/**
 * Folder of a new chat in Inbox: creation time (server local time, sorts chronologically)
 * plus the start of the session id (unique), e.g. `2026-11-06_15-09-30_4333b6d2`. Not the
 * message text: a title in a path is unreadable after transliteration and leaks the topic.
 */
export function inboxDirFor(id: string, at = new Date()): string {
  const p = (n: number) => String(n).padStart(2, '0');
  const stamp = `${at.getFullYear()}-${p(at.getMonth() + 1)}-${p(at.getDate())}_${p(at.getHours())}-${p(at.getMinutes())}-${p(at.getSeconds())}`;
  let dir = join(config.inboxDir, `${stamp}_${id.replace(/[^a-z0-9]/gi, '').slice(0, 8) || 'chat'}`);
  if (existsSync(dir)) dir = `${dir}-${Date.now().toString(36)}`;
  mkdirSync(dir, { recursive: true });
  return dir;
}

const CODE_EXT = new Set(['.ts', '.tsx', '.js', '.jsx', '.mjs', '.py', '.rs', '.go', '.java', '.kt', '.rb', '.php', '.c', '.cc', '.cpp', '.h', '.cs', '.swift', '.vue', '.svelte', '.html', '.css', '.scss', '.sh', '.sql']);
const MARKERS = ['package.json', 'Cargo.toml', 'pyproject.toml', 'requirements.txt', 'go.mod', 'pom.xml', 'build.gradle', 'Makefile', 'docker-compose.yml', 'compose.yaml', 'Dockerfile', '.git'];

function countCode(dir: string, depth = 0): number {
  if (depth > 3) return 0;
  let n = 0;
  let entries: string[] = [];
  try {
    entries = readdirSync(dir);
  } catch {
    return 0;
  }
  for (const e of entries) {
    if (e === 'node_modules' || e.startsWith('.')) continue;
    const p = join(dir, e);
    try {
      const st = statSync(p);
      if (st.isDirectory()) n += countCode(p, depth + 1);
      else if (CODE_EXT.has(extname(e))) n++;
    } catch {}
    if (n > 50) break;
  }
  return n;
}

export function suggestion(s: Session): ProjectSuggestion {
  const name = slug(s.title);
  if (s.projectId || !s.cwd || !existsSync(s.cwd)) return { suggest: false, reason: null, name };
  const marker = MARKERS.find((m) => existsSync(join(s.cwd!, m)));
  if (marker) return { suggest: true, reason: `found ${marker}`, name };
  const n = countCode(s.cwd);
  if (n >= 2) return { suggest: true, reason: `${n} source files`, name };
  return { suggest: false, reason: null, name };
}

/** Short conversation summary — passed to the agent if it can't continue the session in a new folder. */
function handoffText(sessionId: string, from: string, to: string): string {
  const evs = events
    .list(sessionId)
    .filter((e) => e.kind === 'user' || e.kind === 'text')
    .slice(-12)
    .map((e) => `${e.kind === 'user' ? 'User' : 'Assistant'}: ${(e as { text: string }).text.slice(0, 1500)}`);
  return `This conversation was moved from the inbox folder ${from} into the project folder ${to}. All files are now in the current directory. Recent conversation for context:\n\n${evs.join('\n\n')}\n\n---\n`;
}

export async function convertToProject(s: Session, rawName: string): Promise<Project> {
  if (s.projectId || !s.cwd) throw new HttpError(400, 'Only inbox chats can be converted');
  const name = rawName.trim() || s.title;
  const dest = join(projectsDir(), slug(name));
  if (existsSync(dest)) throw new HttpError(409, `Folder already exists: ${dest}`);
  const from = s.cwd;

  try {
    renameSync(from, dest);
  } catch {
    cpSync(from, dest, { recursive: true });
    rmSync(from, { recursive: true, force: true });
  }

  if (!existsSync(join(dest, '.git'))) await sh('git', ['init', '-q', dest]);
  if (!existsSync(join(dest, 'AGENTS.md')))
    writeFileSync(join(dest, 'AGENTS.md'), `# ${name}\n\nProject rules for any agent working here.\n`);

  const project = projects.create(name, dest);
  createManifest(project);
  sessions.moveToProject(s.id, project.id);

  // continue the same session if the agent can carry its transcript over to the new path,
  // otherwise pass the conversation summary as the next message
  const resumed = findAgent(s.agent.kind)?.relocate?.(s, from, dest) ?? false;
  if (!resumed) {
    sessions.update(s.id, { agentSessionId: null as unknown as string });
    sessions.setHandoff(s.id, handoffText(s.id, from, dest));
  }
  return projects.get(project.id)!;
}
