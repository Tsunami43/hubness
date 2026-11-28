import { existsSync, mkdirSync, readdirSync, realpathSync, rmSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, dirname, isAbsolute, join, resolve, sep } from 'node:path';
import type { GroupDetail, HubSettings, JobInfo, PermissionMode, Project, ProjectDetail, ProjectGroup, RepoInfo } from '@hubness/shared';
import { childEnv, config, repoRoot } from './config.ts';
import { groups, kv, projects, sessions, DEFAULT_INSTRUCTIONS_KEY, SESSION_INSTRUCTIONS_KEY } from './db/index.ts';
import { HttpError } from './errors.ts';
import * as git from './git.ts';
import { gitAuthFor } from './git-auth.ts';
import { isCloneUrl, repoNameFromUrl, resolveProvider, stripCredentials } from './git-providers.ts';
import { groupContext, type RepoContext } from './context.ts';
import { repoLanguages } from './languages.ts';
import { groupWorkspace, projectWorkspace, removeWorkspaces } from './workspaces/index.ts';
import { shq, startJob } from './terminal.ts';

/**
 * Catalog of projects and groups (model as in Replit/Coder: projects are what the hub
 * creates and stores itself).
 * - Project = exactly one git repository. Appears only explicitly: a new empty one
 *   (`git init`), a clone by URL, or an import of an existing repository.
 * - The projects folder is fixed: ~/hub/projects (HUBNESS_HOME/projects). New projects
 *   (clone, empty, a chat moved to a project) are placed there; imported repositories stay
 *   where they are. The hub does NOT scan it on its own; repositories found there are only
 *   offered for import.
 * - Group = a set of projects (many-to-many), with its own workspace, sessions and
 *   (optionally) its own storage folder for projects created from the group.
 */

const SKIP = new Set(['node_modules', 'target', 'dist', 'build', 'vendor', '__pycache__', '.venv', 'venv']);
const MAX_DEPTH = 3;

// ---------- settings ----------

const PERMISSION_KEY = 'settings.defaultPermission';
const PERMISSION_MODES: PermissionMode[] = ['ask', 'acceptEdits', 'bypass', 'plan'];

export function settings(): HubSettings {
  const perm = kv.get<PermissionMode>(PERMISSION_KEY)?.value;
  return {
    projectsRoot: config.projectsDir,
    dataDir: config.dataDir,
    defaultPermission: perm && PERMISSION_MODES.includes(perm) ? perm : 'ask',
    defaultInstructions: kv.get<string>(DEFAULT_INSTRUCTIONS_KEY)?.value ?? '',
    sessionInstructions: sessionInstructions(),
  };
}

/** Instructions added to every session, whatever its scope (project, group, chat). */
export const sessionInstructions = () => kv.get<string>(SESSION_INSTRUCTIONS_KEY)?.value ?? '';

export function setSessionInstructions(text: string): HubSettings {
  if (text.length > 20_000) throw new HttpError(400, 'sessionInstructions: at most 20000 characters');
  kv.set(SESSION_INSTRUCTIONS_KEY, text.trim());
  return settings();
}

/** Instructions new projects start with (existing projects keep theirs). */
export function setDefaultInstructions(text: string): HubSettings {
  if (text.length > 20_000) throw new HttpError(400, 'defaultInstructions: at most 20000 characters');
  kv.set(DEFAULT_INSTRUCTIONS_KEY, text.trim());
  return settings();
}

export function setDefaultPermission(mode: string): HubSettings {
  if (!PERMISSION_MODES.includes(mode as PermissionMode)) throw new HttpError(400, 'defaultPermission: ask | acceptEdits | bypass | plan');
  kv.set(PERMISSION_KEY, mode);
  return settings();
}

/** Where new projects go (also a chat on "Move to project"). */
export const projectsDir = () => config.projectsDir;

// ---------- repository discovery ----------

const isRepoDir = (dir: string) => existsSync(join(dir, '.git'));

/** git repositories in a folder (up to 3 levels; we don't descend into a repository). */
export function scanRepos(root: string): string[] {
  const out: string[] = [];
  const walk = (dir: string, depth: number) => {
    if (isRepoDir(dir)) {
      out.push(dir);
      return;
    }
    if (depth >= MAX_DEPTH) return;
    let entries: string[] = [];
    try {
      entries = readdirSync(dir, { withFileTypes: true })
        .filter((d) => d.isDirectory() && !d.name.startsWith('.') && !SKIP.has(d.name))
        .map((d) => d.name);
    } catch {
      return;
    }
    for (const name of entries) walk(join(dir, name), depth + 1);
  };
  walk(root, 0);
  return out.sort();
}

/** All hub projects (no disk scan). If the folder is gone, the project is marked missing. */
export function listProjects(): Project[] {
  const list = projects.list().map((p) => (existsSync(p.path) ? p : { ...p, missing: true }));
  refreshRepoMetaInBackground(list.filter((p) => !p.missing));
  return list;
}

/**
 * Repositories that can be imported: found in the folder (by default the
 * projects folder) and not yet added to the hub. Only a suggestion — creates nothing.
 */
export function discoverRepos(path?: string): { root: string; repos: { path: string; name: string }[] } {
  const root = path?.trim() ? resolve(path.trim()) : config.projectsDir;
  if (!existsSync(root) || !statSync(root).isDirectory()) throw new HttpError(400, 'Folder not found');
  const known = new Set(projects.list().map((p) => p.path));
  return { root, repos: scanRepos(root).filter((r) => !known.has(r)).map((r) => ({ path: r, name: basename(r) })) };
}

/** Import existing repositories (explicit user choice). Already added ones are not duplicated. */
export function importProjects(b: { paths?: unknown; groupId?: unknown }): Project[] {
  if (!Array.isArray(b.paths) || !b.paths.length || b.paths.some((x) => typeof x !== 'string')) throw new HttpError(400, 'paths must be a non-empty list of folders');
  const group = typeof b.groupId === 'string' && b.groupId ? groupOr404(b.groupId) : null;
  const dirs = (b.paths as string[]).map((raw) => {
    const dir = resolve(raw.trim());
    if (!existsSync(dir) || !statSync(dir).isDirectory()) throw new HttpError(400, `Folder not found: ${dir}`);
    if (!isRepoDir(dir)) throw new HttpError(400, `Not a git repository: ${dir}`);
    return dir;
  });
  const out = dirs.map((dir) => projects.byPath(dir) ?? projects.create(basename(dir), dir, 'normal'));
  if (group) addToGroup(group, out.map((p) => p.id));
  return out;
}

/** Checks before removing a project from the hub (with or without its files). */
export function assertRemovable(id: string, isActive: (sessionId: string) => boolean): Project {
  const p = projects.get(id);
  if (!p) throw new HttpError(404, 'Project not found');
  if (p.kind === 'system') throw new HttpError(400, 'The system project can’t be removed');
  if (sessions.list({ projectId: id, archived: 'all' }).some((s) => isActive(s.id))) throw new HttpError(409, 'An agent is working in this project — stop it first');
  return p;
}

/** Remove a project from the hub. Repository files are untouched; project sessions are deleted. */
export function removeProject(id: string, isActive: (sessionId: string) => boolean) {
  assertRemovable(id, isActive);
  removeWorkspaces('project', id);
  projects.delete(id);
}

const real = (path: string) => {
  try {
    return realpathSync(path);
  } catch {
    return resolve(path);
  }
};
const inside = (child: string, parent: string) => child === parent || child.startsWith(parent + sep);

/**
 * The folder "delete with files" may remove: the project's own repository and nothing wider.
 * null — the folder is already gone (only the hub record is left to remove). Refuses system
 * folders (/, home, the hub's folders, the hub's own code and data), folders that are not a
 * git repository root, and folders that contain another project.
 */
export function deletableFolder(p: Project): string | null {
  if (!existsSync(p.path)) return null;
  const dir = real(p.path);
  const guarded = [homedir(), config.hubHome, config.projectsDir, config.inboxDir, config.homeProjectDir].map(real);
  if (dir.split(sep).filter(Boolean).length < 2 || guarded.includes(dir)) throw new HttpError(400, `Refusing to delete ${dir}: a system folder`);
  if (inside(real(repoRoot), dir) || inside(real(config.dataDir), dir)) throw new HttpError(400, `Refusing to delete ${dir}: it contains the hub itself`);
  if (!statSync(dir).isDirectory() || !existsSync(join(dir, '.git'))) throw new HttpError(400, `Refusing to delete ${dir}: not the root of a git repository`);
  const nested = projects.list().find((o) => o.id !== p.id && inside(real(o.path), dir));
  if (nested) throw new HttpError(409, `Refusing to delete ${dir}: project "${nested.name}" lives inside`);
  return dir;
}

/** Delete the project's folder from disk (after deletableFolder). Irreversible. */
export function deleteFolder(dir: string) {
  try {
    rmSync(dir, { recursive: true, force: true, maxRetries: 3 });
  } catch (e) {
    throw new HttpError(500, `Could not delete ${dir}: ${(e as Error).message}`);
  }
}

// repository info (provider, remote, default branch) for lists —
// once per server start, in the background, one at a time
const metaRefreshed = new Set<string>();
let metaQueue: Promise<void> = Promise.resolve();
function refreshRepoMetaInBackground(list: Project[]) {
  const todo = list.filter((p) => p.kind === 'normal' && !metaRefreshed.has(p.id));
  for (const p of todo) metaRefreshed.add(p.id);
  if (!todo.length) return;
  metaQueue = metaQueue.then(async () => {
    for (const p of todo) await repoInfo(p).catch(() => {});
  });
}

// ---------- project ----------

export function projectOr404(id: string): Project {
  const p = projects.get(id);
  if (!p) throw new HttpError(404, 'Project not found');
  if (!existsSync(p.path)) throw new HttpError(410, `Project folder not found: ${p.path}`);
  return p;
}

/** Repository info from git; cached in the project (provider, URL, default branch). */
export async function repoInfo(p: Project): Promise<RepoInfo> {
  const b = await git.repoBasics(p.path);
  const remoteUrl = b.remoteUrl ? stripCredentials(b.remoteUrl) : null;
  const prov = resolveProvider(remoteUrl);
  if (p.gitProvider !== (prov?.id ?? null) || p.repoUrl !== remoteUrl || p.defaultBranch !== b.defaultBranch)
    projects.setRepoMeta(p.id, { gitProvider: prov?.id ?? null, repoUrl: remoteUrl, defaultBranch: b.defaultBranch });
  const links = prov?.adapter && prov.remote ? prov.adapter.links(prov.remote) : null;
  const languages = b.isRepo ? await repoLanguages(p.path) : [];
  const main = languages[0] ? { name: languages[0].name, color: languages[0].color } : null;
  if (JSON.stringify(main) !== JSON.stringify(p.language)) projects.setLanguage(p.id, main);
  return {
    languages,
    isRepo: b.isRepo,
    provider: prov?.id ?? null,
    providerLabel: prov?.label ?? null,
    remoteUrl,
    branch: b.branch,
    defaultBranch: b.defaultBranch,
    links: links
      ? {
          repo: links.repo,
          branch: b.branch ? links.branch(b.branch) : null,
          newPullRequest: b.branch && b.defaultBranch && b.branch !== b.defaultBranch ? links.newPullRequest(b.branch, b.defaultBranch) : null,
        }
      : null,
  };
}

export async function projectDetail(id: string): Promise<ProjectDetail> {
  const p = projectOr404(id);
  const repo = await repoInfo(p);
  const fresh = projects.get(id)!;
  return {
    ...fresh,
    repo,
    groups: fresh.groupIds.map((gid) => groups.get(gid)).filter((g): g is ProjectGroup => Boolean(g)).map((g) => ({ id: g.id, name: g.name })),
    workspace: projectWorkspace(fresh),
  };
}

const LIMITS = { name: 120, description: 4000, instructions: 20_000 };

function text(v: unknown, field: keyof typeof LIMITS): string | undefined {
  if (v === undefined) return undefined;
  if (typeof v !== 'string') throw new HttpError(400, `${field} must be a string`);
  if (v.length > LIMITS[field]) throw new HttpError(400, `${field} is too long (max ${LIMITS[field]})`);
  return field === 'name' ? v.trim() : v;
}

export function updateProject(id: string, b: { name?: unknown; description?: unknown; instructions?: unknown }): Project {
  projectOr404(id);
  const name = text(b.name, 'name');
  if (name === '') throw new HttpError(400, 'Name is required');
  return projects.update(id, { name, description: text(b.description, 'description'), instructions: text(b.instructions, 'instructions') })!;
}

/** Where to put a new project: the group folder (if set) or the projects folder. */
function storageDir(group: ProjectGroup | null): string {
  return group?.folder ?? config.projectsDir;
}

function targetDir(name: string, group: ProjectGroup | null): { dir: string; slug: string } {
  const root = storageDir(group);
  const slug = name.trim().replace(/[^\w.-]+/g, '-').replace(/^[-.]+|-+$/g, '');
  if (!slug) throw new HttpError(400, 'Name is required');
  const dir = join(root, slug);
  if (existsSync(dir)) throw new HttpError(409, `A folder named “${slug}” already exists in ${root}`);
  return { dir, slug };
}

const groupFrom = (v: unknown) => (typeof v === 'string' && v ? groupOr404(v) : null);

/** A new empty project in storage (group or projects folder): folder + git init. */
export async function createRepo(b: { name?: unknown; groupId?: unknown }): Promise<Project> {
  const group = groupFrom(b.groupId);
  const { dir, slug } = targetDir(typeof b.name === 'string' ? b.name : '', group);
  mkdirSync(dir, { recursive: true });
  const { execFileSync } = await import('node:child_process');
  execFileSync('git', ['init', '-q', dir], { env: childEnv() });
  const p = projects.create(slug, dir, 'normal');
  if (group) addToGroup(group, [p.id]);
  return projects.get(p.id)!;
}

/**
 * Clone a repository into storage as a terminal task: progress, password prompts
 * or host key confirmation are visible. The project appears when git
 * clone has finished successfully (not earlier).
 */
export function cloneRepo(b: { url?: unknown; name?: unknown; groupId?: unknown; cols?: number; rows?: number }): JobInfo {
  const url = typeof b.url === 'string' ? b.url.trim() : '';
  if (!isCloneUrl(url)) throw new HttpError(400, 'Use an https://, ssh:// or git@host:path repository URL');
  const name = (typeof b.name === 'string' && b.name.trim()) || repoNameFromUrl(url);
  if (!name) throw new HttpError(400, 'Couldn’t derive a folder name from the URL');
  const group = groupFrom(b.groupId);
  const { dir, slug } = targetDir(name, group);
  mkdirSync(dirname(dir), { recursive: true });
  // a login stored for the host (System → Git access) is used; otherwise git asks in the terminal
  const auth = gitAuthFor([url]);
  return startJob({
    title: `Clone ${slug}`,
    command: ['git', ...auth.args.map(shq), 'clone', '--progress', '--', shq(url), shq(dir)].join(' '),
    env: auth.env,
    cols: b.cols,
    rows: b.rows,
    onExit: (code) => {
      if (code !== 0 || !isRepoDir(dir)) return;
      const p = projects.byPath(dir) ?? projects.create(slug, dir, 'normal');
      const g = group && groups.get(group.id);
      if (g) addToGroup(g, [p.id]);
    },
  });
}

// ---------- groups ----------

function membersOf(ids: unknown): Project[] {
  if (!Array.isArray(ids) || ids.some((x) => typeof x !== 'string')) throw new HttpError(400, 'projectIds must be a list of project ids');
  const unique = [...new Set(ids as string[])];
  const list = unique.map((id) => {
    const p = projects.get(id);
    if (!p || p.kind !== 'normal') throw new HttpError(400, `Unknown project: ${id}`);
    return p;
  });
  return list;
}

export function groupOr404(id: string): ProjectGroup {
  const g = groups.get(id);
  if (!g) throw new HttpError(404, 'Group not found');
  return g;
}

function addToGroup(g: ProjectGroup, ids: string[]) {
  groups.update(g.id, { projectIds: [...new Set([...g.projectIds, ...ids])] });
}

/** Group storage folder: an absolute path or empty (= the projects folder). */
function folderOf(v: unknown): string | null | undefined {
  if (v === undefined) return undefined;
  if (v === null || v === '') return null;
  if (typeof v !== 'string' || !isAbsolute(v.trim())) throw new HttpError(400, 'folder must be an absolute path');
  const abs = resolve(v.trim());
  if (existsSync(abs) && !statSync(abs).isDirectory()) throw new HttpError(400, 'folder is not a directory');
  return abs;
}

export const groupProjects = (g: ProjectGroup): Project[] =>
  g.projectIds.map((id) => projects.get(id)).filter((p): p is Project => Boolean(p));

export function groupDetail(id: string): GroupDetail {
  const g = groupOr404(id);
  const list = groupProjects(g);
  return { ...g, projects: list, workspace: groupWorkspace(g.id, list) };
}

export function createGroup(b: { name?: unknown; description?: unknown; instructions?: unknown; folder?: unknown; projectIds?: unknown }): GroupDetail {
  const name = text(b.name ?? '', 'name');
  if (!name) throw new HttpError(400, 'Name is required');
  const members = membersOf(b.projectIds ?? []);
  const g = groups.create({
    name,
    description: text(b.description, 'description'),
    instructions: text(b.instructions, 'instructions'),
    folder: folderOf(b.folder) ?? null,
    projectIds: members.map((m) => m.id),
  });
  return groupDetail(g.id);
}

export function updateGroup(id: string, b: { name?: unknown; description?: unknown; instructions?: unknown; folder?: unknown; projectIds?: unknown }): GroupDetail {
  groupOr404(id);
  const name = text(b.name, 'name');
  if (name === '') throw new HttpError(400, 'Name is required');
  groups.update(id, {
    name,
    description: text(b.description, 'description'),
    instructions: text(b.instructions, 'instructions'),
    folder: folderOf(b.folder),
    projectIds: b.projectIds === undefined ? undefined : membersOf(b.projectIds).map((m) => m.id),
  });
  return groupDetail(id);
}

/** Deletes a group, its workspace and group sessions. Projects and their sessions are untouched. */
export function deleteGroup(id: string, isActive: (sessionId: string) => boolean) {
  groupOr404(id);
  if (sessions.list({ groupId: id, archived: 'all' }).some((s) => isActive(s.id))) throw new HttpError(409, 'An agent is working in this group — stop it first');
  removeWorkspaces('group', id);
  groups.delete(id);
}

/**
 * Context the agent receives in a group session. Synchronous: takes
 * repository info from the project cache (refreshed by repoInfo).
 */
export function groupAgentContext(g: ProjectGroup): string {
  const repos: RepoContext[] = groupProjects(g).map((p) => ({
    project: p,
    repo: { branch: null, defaultBranch: p.defaultBranch, remoteUrl: p.repoUrl, providerLabel: resolveProvider(p.repoUrl)?.label ?? null },
  }));
  return groupContext(g, repos);
}

/** Refresh the cached repository info of a group (for the context preview). */
export async function refreshGroupRepos(g: ProjectGroup) {
  await Promise.all(groupProjects(g).filter((p) => existsSync(p.path)).map((p) => repoInfo(p)));
}
