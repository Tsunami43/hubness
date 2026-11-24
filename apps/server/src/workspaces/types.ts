import type { Project, Workspace, WorkspaceCapabilities } from '@hubness/shared';

/** Whose environment it is: a single project or a group of projects. */
export type WorkspaceScope = { kind: 'project'; id: string; project: Project } | { kind: 'group'; id: string; projects: Project[] };

/**
 * Execution environment provider. Currently only local; Docker, Kubernetes,
 * a remote VM, an SSH machine — future implementations of the same contract.
 */
export interface WorkspaceProvider {
  id: string;
  label: string;
  capabilities: WorkspaceCapabilities;
  /** Prepare the environment (create, update the set of repositories) and return the path and status. */
  ensure(scope: WorkspaceScope): { path: string; strategy: Workspace['strategy']; status: Workspace['status']; statusDetail: string | null };
  /** Delete the provider's environment (don't touch the user's repositories). */
  remove(scope: { kind: WorkspaceScope['kind']; id: string }, path: string): void;
}
