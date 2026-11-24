import type { Project, Workspace } from '@hubness/shared';
import { workspaces } from '../db/index.ts';
import { localProvider } from './local.ts';
import type { WorkspaceProvider, WorkspaceScope } from './types.ts';

/** Workspace provider registry. New provider = WorkspaceProvider implementation + an entry here. */
const providers = new Map<string, WorkspaceProvider>([[localProvider.id, localProvider]]);
export const DEFAULT_PROVIDER = localProvider.id;

export function workspaceProvider(id: string): WorkspaceProvider {
  const p = providers.get(id);
  if (!p) throw new Error(`Unknown workspace provider: ${id}`);
  return p;
}

/** Prepare the scope's workspace and save its state. */
export function ensureWorkspace(scope: WorkspaceScope, providerId = DEFAULT_PROVIDER): Workspace {
  const provider = workspaceProvider(providerId);
  const r = provider.ensure(scope);
  const row = workspaces.upsert({ scope: scope.kind, scopeId: scope.id, provider: provider.id, ...r });
  return { ...row, capabilities: provider.capabilities };
}

export const projectWorkspace = (project: Project) => ensureWorkspace({ kind: 'project', id: project.id, project });
export const groupWorkspace = (id: string, projects: Project[]) => ensureWorkspace({ kind: 'group', id, projects });

export function removeWorkspaces(scope: WorkspaceScope['kind'], scopeId: string) {
  for (const p of providers.values()) {
    const row = workspaces.find(scope, scopeId, p.id);
    if (row) p.remove({ kind: scope, id: scopeId }, row.path);
  }
  workspaces.deleteScope(scope, scopeId);
}
