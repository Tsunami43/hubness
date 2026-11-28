import { spawnSync } from 'node:child_process';
import { appendFileSync, existsSync, lstatSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { extname, join, resolve } from 'node:path';
import type { Attachment } from '@hubness/shared';
import { HttpError } from './errors.ts';

/**
 * Files attached to messages. They live in the session's working folder, one folder per
 * session (`.hub/attachments/<session id>/`), not in the hub data: the sandbox hides the hub
 * data from agents, and an agent opens a video, a PDF or an archive with its own tools by
 * path. Inside a repository the folder is excluded locally (`.git/info/exclude`): nothing to
 * commit by accident, .gitignore stays untouched.
 *
 * Archiving a session keeps its files (it can come back); deleting it removes its folder.
 * Files from before the per-session folders lie flat in `.hub/attachments/`: they are still
 * served, and deleting a session removes the ones its messages refer to.
 */

export const MAX_ATTACHMENT = 20 * 1024 * 1024;
const DIR = join('.hub', 'attachments');
const EXCLUDE_LINE = '/.hub/attachments/';

/** Images a model takes directly (Claude and ACP agents); larger ones go by path only. */
const MODEL_IMAGES = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp']);
const MODEL_IMAGE_MAX = 5 * 1024 * 1024;
export const isModelImage = (a: Pick<Attachment, 'mime' | 'size'>) => MODEL_IMAGES.has(a.mime) && a.size <= MODEL_IMAGE_MAX;

const MIME_BY_EXT: Record<string, string> = {
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.svg': 'image/svg+xml',
  '.mp4': 'video/mp4',
  '.webm': 'video/webm',
  '.mov': 'video/quicktime',
  '.pdf': 'application/pdf',
  '.txt': 'text/plain',
  '.md': 'text/markdown',
  '.json': 'application/json',
  '.zip': 'application/zip',
};

const rootOf = (cwd: string) => join(cwd, DIR);
/** The session's own folder (session ids are UUIDs; anything else is made path-safe). */
const dirOf = (cwd: string, sessionId: string) => join(rootOf(cwd), sessionId.replace(/[^\w-]/g, '_') || '_');
const validId = (id: string) => Boolean(id) && !id.includes('/') && !id.includes('\\') && !id.startsWith('.');
const isFile = (path: string) => existsSync(path) && lstatSync(path).isFile();

/** Keep the attachments folder out of git, locally (works for worktrees too). */
function excludeFromGit(cwd: string) {
  const r = spawnSync('git', ['-C', cwd, 'rev-parse', '--show-toplevel', '--git-path', 'info/exclude'], { encoding: 'utf8', timeout: 5000 });
  if (r.status !== 0) return;
  const [top, rel] = r.stdout.trim().split('\n');
  // only when the session folder is the repository root (a group workspace is not a repository)
  if (!top || !rel || resolve(top) !== resolve(cwd)) return;
  const file = resolve(cwd, rel);
  try {
    const cur = existsSync(file) ? readFileSync(file, 'utf8') : '';
    if (cur.split('\n').some((l) => l.trim() === EXCLUDE_LINE)) return;
    mkdirSync(join(file, '..'), { recursive: true });
    appendFileSync(file, `${cur && !cur.endsWith('\n') ? '\n' : ''}# hubness: files attached in chats\n${EXCLUDE_LINE}\n`);
  } catch {}
}

/** A file name safe on every OS and in a shell command line. */
function safeName(name: string): string {
  const base = name.replace(/[/\\]/g, '_').replace(/[^\p{L}\p{N}._ -]/gu, '_').replace(/\s+/g, ' ').trim().slice(0, 100);
  return base && !/^\.+$/.test(base) ? base : 'file';
}

export function saveAttachment(cwd: string, sessionId: string, name: string, mime: string, bytes: Uint8Array): Attachment {
  if (!bytes.length) throw new HttpError(400, 'Empty file');
  if (bytes.length > MAX_ATTACHMENT) throw new HttpError(413, `The file is larger than ${MAX_ATTACHMENT / 1024 / 1024} MB`);
  const dir = dirOf(cwd, sessionId);
  mkdirSync(dir, { recursive: true });
  excludeFromGit(cwd);
  const clean = safeName(name);
  const id = `${randomUUID().slice(0, 8)}-${clean}`;
  const path = join(dir, id);
  writeFileSync(path, bytes);
  const type = /^[\w.+-]+\/[\w.+-]+$/.test(mime) ? mime : (MIME_BY_EXT[extname(clean).toLowerCase()] ?? 'application/octet-stream');
  return { id, name: clean, mime: type, size: bytes.length, path };
}

/** Attachments of a message by id: only files really present in this session's folder. */
export function resolveAttachments(cwd: string, sessionId: string, list: unknown): Attachment[] {
  if (list === undefined || list === null) return [];
  if (!Array.isArray(list) || list.length > 20) throw new HttpError(400, 'attachments must be a list of up to 20 items');
  return list.map((a) => {
    const id = typeof a === 'object' && a && typeof (a as Attachment).id === 'string' ? (a as Attachment).id : '';
    const path = attachmentPath(cwd, sessionId, id);
    const st = statSync(path);
    const name = typeof (a as Attachment).name === 'string' ? safeName((a as Attachment).name) : id;
    const mime = typeof (a as Attachment).mime === 'string' && /^[\w.+-]+\/[\w.+-]+$/.test((a as Attachment).mime) ? (a as Attachment).mime : 'application/octet-stream';
    return { id, name, mime, size: st.size, path };
  });
}

/**
 * Absolute path of an attachment: the session's folder, else a file from before the
 * per-session folders. 404 for anything outside the folder or missing.
 */
export function attachmentPath(cwd: string, sessionId: string, id: string): string {
  if (!validId(id)) throw new HttpError(404, 'Attachment not found');
  for (const path of [join(dirOf(cwd, sessionId), id), join(rootOf(cwd), id)]) if (existsSync(path) && statSync(path).isFile()) return path;
  throw new HttpError(404, 'Attachment not found');
}

/**
 * The files of a deleted session: its folder, plus the flat files from before the
 * per-session folders that its messages refer to (`legacyIds`). Best effort — a file that
 * can't be removed stays.
 */
export function removeSessionAttachments(cwd: string, sessionId: string, legacyIds: string[] = []) {
  try {
    rmSync(dirOf(cwd, sessionId), { recursive: true, force: true });
  } catch {}
  for (const id of legacyIds) {
    if (!validId(id)) continue;
    const path = join(rootOf(cwd), id);
    try {
      if (isFile(path)) rmSync(path, { force: true });
    } catch {}
  }
}

/** The line the agent reads: where the files are (it opens them by path). */
export function attachmentsNote(list: Attachment[]): string {
  if (!list.length) return '';
  const kb = (n: number) => (n < 1024 * 1024 ? `${Math.max(1, Math.round(n / 1024))} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`);
  return `\n\nAttached files (open them by path if needed):\n${list.map((a) => `- ${a.path} (${a.mime}, ${kb(a.size)})`).join('\n')}`;
}
