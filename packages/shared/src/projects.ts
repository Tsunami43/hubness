// Projects, groups, workspaces, hub settings, run commands and environment, the manifest.

import type { PermissionMode } from './agents.ts';
import type { JobInfo } from './jobs.ts';

/** Repository git hosting. generic — any other git server (no web links). */
export type GitProviderId = 'github' | 'gitlab' | 'bitbucket' | 'generic';

/**
 * A project is exactly one git repository. Several repositories are combined by
 * a group (ProjectGroup), not by a project.
 */
export interface Project {
  id: string;
  name: string;
  /** Repository folder (agent cwd). */
  path: string;
  /** normal — a git repository added to the hub; system — the system project home. */
  kind: 'normal' | 'system';
  description: string;
  /** Instructions for AI agents (supplement the repository's AGENTS.md). */
  instructions: string;
  gitProvider: GitProviderId | null;
  /** Remote origin URL (without credentials). */
  repoUrl: string | null;
  defaultBranch: string | null;
  /** Groups the project belongs to. */
  groupIds: string[];
  /** Primary language (cached from git, for lists). */
  language: { name: string; color: string | null } | null;
  /** The repository folder disappeared from disk (only in the project list). */
  missing?: boolean;
  createdAt: string;
  updatedAt: string;
}

/** Project repository info (read from git on request). */
export interface RepoInfo {
  isRepo: boolean;
  provider: GitProviderId | null;
  providerLabel: string | null;
  remoteUrl: string | null;
  branch: string | null;
  defaultBranch: string | null;
  /** Provider web links (absent for generic). */
  links: { repo: string; branch: string | null; newPullRequest: string | null } | null;
  /** Languages by bytes (like the language bar on GitHub; colors from GitHub Linguist). */
  languages: RepoLanguage[];
}

export interface RepoLanguage {
  name: string;
  color: string | null;
  percent: number;
}

export interface ProjectDetail extends Project {
  repo: RepoInfo;
  groups: { id: string; name: string }[];
  workspace: Workspace;
}

/** Logical group of projects: a product, a system, a set of microservices. */
export interface ProjectGroup {
  id: string;
  name: string;
  description: string;
  /** Shared AI instructions for the group's sessions. */
  instructions: string;
  /** Storage folder for the group's new projects; null — the shared projects folder. */
  folder: string | null;
  /** Member projects in order (many-to-many: a project can be in several groups). */
  projectIds: string[];
  createdAt: string;
  updatedAt: string;
}

export interface GroupDetail extends ProjectGroup {
  projects: Project[];
  workspace: Workspace;
}

/** Workspace provider capabilities — the UI shows only what is supported. */
export interface WorkspaceCapabilities {
  /** start / stop / restart (the local provider has none — the environment is always available). */
  lifecycle: boolean;
  /** Repositories are cloned into the workspace (rather than used in place). */
  clones: boolean;
}

/** Execution environment: where the code lives and where the agent and tools run. */
export interface Workspace {
  id: string;
  scope: 'project' | 'group';
  scopeId: string;
  /** Provider: currently only local; later docker, kubernetes, ssh… */
  provider: string;
  /** single — one repository; multi — several repositories of a group. */
  strategy: 'single' | 'multi';
  path: string;
  status: 'ready' | 'missing' | 'error';
  statusDetail: string | null;
  capabilities: WorkspaceCapabilities;
  createdAt: string;
  updatedAt: string;
}

/** Hub settings. */
export interface HubSettings {
  /** Storage folder for new projects. null — not set yet. */
  projectsRoot: string;
  /** Permission mode a new session starts with (changeable per session). */
  defaultPermission: PermissionMode;
  /** Instructions for AI agents every new project starts with (Settings → Agent). */
  defaultInstructions: string;
  /** Instructions added to every agent session — project, group or chat (Settings → Agent). Read live, not copied. */
  sessionInstructions: string;
}

// ---------- run, preview, shell, secrets, environment ----------

/** Project run command (like Run/Workflows in Replit). Stored in the hub, not in the repository. */
export interface RunConfig {
  id: string;
  projectId: string;
  name: string;
  command: string;
  /** App port for preview; null — detect from the output (http://localhost:5173). */
  port: number | null;
  /** The primary command — the Run button starts it. */
  primary: boolean;
  /**
   * Needs Docker (docker compose…): the command runs without the sandbox — the
   * Docker socket is hidden inside it. Docker access ≈ root on the server.
   */
  docker: boolean;
  position: number;
}

export interface RunState extends RunConfig {
  /** Current or last run (terminal task). */
  job: JobInfo | null;
  /** Port: from the setting or detected in the output. */
  activePort: number | null;
  /** Preview proxy port on the hub (same host as the hub). */
  previewPort: number | null;
}

/** Run command suggestion (package.json, Makefile, …). */
export interface RunSuggestion {
  name: string;
  command: string;
  source: string;
  port?: number | null;
  docker?: boolean;
}

/**
 * Where Run and Shell execute and how the agent is isolated:
 * - environment: local — on the server; docker — in the project container;
 * - sandbox: the agent, Run and Shell on the server run in a bubblewrap sandbox
 *   (only the scope's repositories are visible, no other projects, keys or docker.sock);
 * - shareSecretsWithAgents: project secrets are passed into the agent's environment.
 */
export interface ProjectRuntime {
  environment: 'local' | 'docker';
  /** Container image (environment = docker). */
  image: string;
  sandbox: boolean;
  shareSecretsWithAgents: boolean;
}

export interface EnvironmentStatus {
  runtime: ProjectRuntime;
  /** Availability of the mechanisms on the server. */
  dockerAvailable: boolean;
  sandboxAvailable: boolean;
  /** Container state (environment = docker). */
  container: { name: string; state: 'running' | 'stopped' | 'absent'; image: string | null } | null;
  /** Image from .devcontainer/devcontainer.json, if present. */
  devcontainerImage: string | null;
}

/** Secret: the value is never sent to the frontend. */
export interface SecretInfo {
  name: string;
  updatedAt: string;
}

// ---- Project manifest .hub/hub.yaml ----

export interface Manifest {
  links: Record<string, string>;
  ssh: string[];
  /** Project Docker containers. */
  services: string[];
  /** Project systemd units. */
  units: string[];
  docs: string[];
  notes?: string;
}

export interface ManifestResponse {
  exists: boolean;
  path: string;
  manifest: Manifest | null;
  error?: string;
  services: { name: string; state: string; status: string }[];
}

export interface ProjectSuggestion {
  suggest: boolean;
  reason: string | null;
  name: string;
}
