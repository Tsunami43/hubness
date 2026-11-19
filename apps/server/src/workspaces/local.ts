import { existsSync, lstatSync, mkdirSync, readlinkSync, readdirSync, rmSync, symlinkSync, unlinkSync } from 'node:fs';
import { basename, join, resolve, sep } from 'node:path';
import { config } from '../config.ts';
import type { WorkspaceProvider } from './types.ts';

/**
 * Local provider: the code already lives on this machine.
 * - project: the workspace is the repository itself (in place, no copies);
 * - group: folder .data/groups/<id>/ with symlinks to the member repositories.
 *   This is the agent's cwd in a group session; the agent gets the real paths separately
 *   (--add-dir / additionalDirectories) so access permissions are evaluated against them.
 * No start or stop: the environment is always available (capabilities.lifecycle = false).
 */

export const groupsDir = () => join(config.dataDir, 'groups');

/** Unique link names: when folder names collide — name-2, name-3… */
export function linkNames(paths: string[]): string[] {
  const used = new Set<string>();
  return paths.map((p) => {
    const base = basename(p);
    let name = base;
    for (let i = 2; used.has(name); i++) name = `${base}-${i}`;
    used.add(name);
    return name;
  });
}

export const localProvider: WorkspaceProvider = {
  id: 'local',
  label: 'This server',
  capabilities: { lifecycle: false, clones: false },

  ensure(scope) {
    if (scope.kind === 'project') {
      const ok = existsSync(scope.project.path);
      return { path: scope.project.path, strategy: 'single', status: ok ? 'ready' : 'missing', statusDetail: ok ? null : 'Repository folder not found' };
    }
    const dir = join(groupsDir(), scope.id);
    mkdirSync(dir, { recursive: true });
    const names = linkNames(scope.projects.map((p) => p.path));
    const want = new Map(names.map((n, i) => [n, scope.projects[i].path]));
    // remove extra/stale links, create needed ones; leave other files alone
    for (const entry of readdirSync(dir)) {
      const p = join(dir, entry);
      if (!lstatSync(p).isSymbolicLink()) continue;
      if (want.get(entry) !== readlinkSync(p)) unlinkSync(p);
    }
    for (const [name, target] of want) if (!existsSync(join(dir, name))) symlinkSync(target, join(dir, name));
    const missing = scope.projects.filter((p) => !existsSync(p.path)).map((p) => p.name);
    return {
      path: dir,
      strategy: 'multi',
      status: missing.length ? 'missing' : 'ready',
      statusDetail: missing.length ? `Missing repositories: ${missing.join(', ')}` : null,
    };
  },

  remove(scope, path) {
    if (scope.kind !== 'group') return; // a project's workspace is the repository itself; don't delete it
    const dir = resolve(path);
    if (dir.startsWith(resolve(groupsDir()) + sep) && existsSync(dir)) rmSync(dir, { recursive: true, force: true });
  },
};
