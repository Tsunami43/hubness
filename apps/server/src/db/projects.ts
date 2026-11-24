import { randomUUID } from 'node:crypto';
import type { GitProviderId, Project, ProjectRuntime } from '@hubness/shared';
import { db, now, type Row } from './connection.ts';
import { DEFAULT_INSTRUCTIONS_KEY, kv } from './kv.ts';

// ---------- projects ----------

const groupIdsOf = (projectId: string): string[] =>
  db.prepare('SELECT group_id FROM group_members WHERE project_id = ?').all(projectId).map((r) => r.group_id as string);

const parseLanguage = (v: unknown): Project['language'] => {
  if (typeof v !== 'string' || !v) return null;
  try {
    return JSON.parse(v) as Project['language'];
  } catch {
    return null;
  }
};

const toProject = (r: Row): Project => ({
  id: r.id as string,
  name: r.name as string,
  path: r.path as string,
  kind: (r.kind === 'system' ? 'system' : 'normal') as Project['kind'],
  description: (r.description as string) ?? '',
  instructions: (r.instructions as string) ?? '',
  gitProvider: ((r.git_provider as string) || null) as GitProviderId | null,
  repoUrl: (r.repo_url as string) || null,
  defaultBranch: (r.default_branch as string) || null,
  groupIds: groupIdsOf(r.id as string),
  language: parseLanguage(r.language),
  createdAt: r.created_at as string,
  updatedAt: r.updated_at as string,
});

export const projects = {
  list(): Project[] {
    return db.prepare('SELECT * FROM projects ORDER BY updated_at DESC').all().map(toProject);
  },
  get(id: string): Project | undefined {
    const r = db.prepare('SELECT * FROM projects WHERE id = ?').get(id);
    return r ? toProject(r) : undefined;
  },
  byPath(path: string): Project | undefined {
    const r = db.prepare('SELECT * FROM projects WHERE path = ?').get(path);
    return r ? toProject(r) : undefined;
  },
  create(name: string, path: string, kind: Project['kind'] = 'normal'): Project {
    const id = randomUUID();
    const t = now();
    // a new normal project starts with the default instructions from Settings → Agent
    const instructions = kind === 'normal' ? (kv.get<string>(DEFAULT_INSTRUCTIONS_KEY)?.value ?? '') : '';
    db.prepare('INSERT INTO projects (id, name, path, kind, instructions, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)').run(
      id,
      name,
      path,
      kind,
      instructions,
      t,
      t,
    );
    return projects.get(id)!;
  },
  system(): Project | undefined {
    const r = db.prepare("SELECT * FROM projects WHERE kind = 'system' LIMIT 1").get();
    return r ? toProject(r) : undefined;
  },
  update(id: string, patch: { name?: string; description?: string; instructions?: string }): Project | undefined {
    const cur = projects.get(id);
    if (!cur) return undefined;
    const next = { ...cur, ...Object.fromEntries(Object.entries(patch).filter(([, v]) => v !== undefined)) };
    db.prepare('UPDATE projects SET name = ?, description = ?, instructions = ?, updated_at = ? WHERE id = ?').run(
      next.name,
      next.description,
      next.instructions,
      now(),
      id,
    );
    return projects.get(id);
  },
  /** Repository info read from git (cached in the DB for lists). */
  setRepoMeta(id: string, meta: { gitProvider: GitProviderId | null; repoUrl: string | null; defaultBranch: string | null }) {
    db.prepare('UPDATE projects SET git_provider = ?, repo_url = ?, default_branch = ? WHERE id = ?').run(
      meta.gitProvider,
      meta.repoUrl,
      meta.defaultBranch,
      id,
    );
  },
  setLanguage(id: string, language: Project['language']) {
    db.prepare('UPDATE projects SET language = ? WHERE id = ?').run(language ? JSON.stringify(language) : null, id);
  },
  touch(id: string) {
    db.prepare('UPDATE projects SET updated_at = ? WHERE id = ?').run(now(), id);
  },
  delete(id: string) {
    db.prepare('DELETE FROM projects WHERE id = ?').run(id);
  },
};

// ---------- project environment (JSON in projects.runtime) ----------

export const projectRuntime = {
  get(projectId: string): Partial<ProjectRuntime> {
    const r = db.prepare('SELECT runtime FROM projects WHERE id = ?').get(projectId);
    try {
      return r?.runtime ? (JSON.parse(r.runtime as string) as Partial<ProjectRuntime>) : {};
    } catch {
      return {};
    }
  },
  set(projectId: string, value: Partial<ProjectRuntime>) {
    db.prepare('UPDATE projects SET runtime = ? WHERE id = ?').run(JSON.stringify(value), projectId);
  },
};
