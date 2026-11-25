import type {
  HarnessExtensionsAll,
  McpCatalogResponse,
  SkillContent,
  SkillCreateInput,
  SkillImportInput,
  SkillImportPreview,
  SkillsResponse,
  AgentConfig,
  AgentKind,
  AgentModelsResponse,
  AgentsResponse,
  ApprovalDecision,
  Attachment,
  BrowseResponse,
  ContainerInfo,
  DockerNetwork,
  DockerVolume,
  EnvironmentStatus,
  FileContent,
  FileWrite,
  FsEntry,
  GitBranch,
  GitLogResponse,
  GitOpResult,
  GitCredentialInfo,
  GitCredentialKind,
  GitStash,
  GitStatus,
  GroupDetail,
  HarnessExtensions,
  HarnessInfo,
  McpResponse,
  McpServerInput,
  RulesFileContent,
  HubSettings,
  JobInfo,
  PackageManagerInfo,
  PackagesResponse,
  PermissionMode,
  Project,
  ProjectDetail,
  ProjectGroup,
  ProjectRuntime,
  ProjectSuggestion,
  RunState,
  RunSuggestion,
  SearchOptions,
  SearchResult,
  SecretInfo,
  Session,
  SystemOverview,
  SystemPlatform,
  UnitInfo,
  User,
} from '@hubness/shared';

import { t } from '../i18n';

/** API error with the HTTP status (e.g. 409 — the file changed on disk). */
export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function req<T>(method: string, url: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${url}`, {
    method,
    // X-Hubness-Request: without it the server rejects mutating requests (CSRF protection)
    headers: { 'X-Hubness-Request': '1', ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}) },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  const data = text ? JSON.parse(text) : null;
  // the session is gone (or the hub just got its first account): show the sign-in screen
  if (res.status === 401) window.dispatchEvent(new Event('hubness:unauthorized'));
  if (!res.ok) throw new ApiError(res.status, data?.error ?? t('api.error', { status: res.status }));
  return data as T;
}

const q = (params: Record<string, string | undefined>) => {
  const s = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined) s.set(k, v);
  const str = s.toString();
  return str ? `?${str}` : '';
};

export const api = {
  agents: () => req<AgentsResponse>('GET', '/agents'),
  settings: () => req<HubSettings>('GET', '/settings'),
  saveSettings: (patch: { defaultPermission?: PermissionMode; defaultInstructions?: string; sessionInstructions?: string }) => req<HubSettings>('PUT', '/settings', patch),
  newRepo: (input: { name: string; groupId?: string }) => req<Project>('POST', '/projects/new-repo', input),
  cloneRepo: (input: { url: string; name?: string; groupId?: string; cols?: number; rows?: number }) => req<JobInfo>('POST', '/projects/clone', input),
  discoverRepos: (path?: string) => req<{ root: string; repos: { path: string; name: string }[] }>('GET', `/projects/discover${q({ path })}`),
  importProjects: (input: { paths: string[]; groupId?: string }) => req<Project[]>('POST', '/projects/import', input),
  removeProject: (id: string) => req<{ ok: true }>('DELETE', `/projects/${id}`),
  /** Remove the project and delete its folder from disk; `confirm` must be the folder name. */
  deleteProjectWithFiles: (id: string, confirm: string) =>
    req<{ ok: true; deleted: string | null }>('DELETE', `/projects/${id}?files=1&confirm=${encodeURIComponent(confirm)}`),
  runs: (id: string) => req<RunState[]>('GET', `/projects/${id}/runs`),
  runSuggestions: (id: string) => req<RunSuggestion[]>('GET', `/projects/${id}/runs/suggest`),
  createRun: (id: string, input: { name: string; command: string; port?: number | null; primary?: boolean; docker?: boolean }) => req<RunState>('POST', `/projects/${id}/runs`, input),
  updateRun: (id: string, rid: string, patch: { name?: string; command?: string; port?: number | null; primary?: boolean; docker?: boolean }) =>
    req<RunState>('PATCH', `/projects/${id}/runs/${rid}`, patch),
  deleteRun: (id: string, rid: string) => req<{ ok: true }>('DELETE', `/projects/${id}/runs/${rid}`),
  startRun: (id: string, rid: string, size: { cols?: number; rows?: number } = {}) => req<RunState>('POST', `/projects/${id}/runs/${rid}/start`, size),
  stopRun: (id: string, rid: string) => req<RunState>('POST', `/projects/${id}/runs/${rid}/stop`),
  shells: (scope: { projectId: string } | { groupId: string }) =>
    req<JobInfo[]>('GET', 'projectId' in scope ? `/projects/${scope.projectId}/shells` : `/groups/${scope.groupId}/shells`),
  previewPort: (id: string, port: number) => req<{ previewPort: number }>('POST', `/projects/${id}/preview`, { port }),
  shell: (scope: { projectId: string } | { groupId: string }, input: { cols?: number; rows?: number; fresh?: boolean }) =>
    req<JobInfo>('POST', 'projectId' in scope ? `/projects/${scope.projectId}/shell` : `/groups/${scope.groupId}/shell`, input),
  environment: (id: string) => req<EnvironmentStatus>('GET', `/projects/${id}/environment`),
  setEnvironment: (id: string, patch: Partial<ProjectRuntime>) => req<EnvironmentStatus>('PUT', `/projects/${id}/environment`, patch),
  removeContainer: (id: string) => req<EnvironmentStatus>('POST', `/projects/${id}/environment/container/remove`),
  secrets: (id: string) => req<{ secrets: SecretInfo[]; unreadable: string[]; keySource: 'env' | 'file' }>('GET', `/projects/${id}/secrets`),
  setSecret: (id: string, name: string, value: string) => req<SecretInfo[]>('PUT', `/projects/${id}/secrets/${encodeURIComponent(name)}`, { value }),
  deleteSecret: (id: string, name: string) => req<SecretInfo[]>('DELETE', `/projects/${id}/secrets/${encodeURIComponent(name)}`),
  groups: () => req<ProjectGroup[]>('GET', '/groups'),
  group: (id: string) => req<GroupDetail>('GET', `/groups/${id}`),
  createGroup: (input: { name: string; description?: string; instructions?: string; folder?: string | null; projectIds: string[] }) => req<GroupDetail>('POST', '/groups', input),
  updateGroup: (id: string, patch: { name?: string; description?: string; instructions?: string; folder?: string | null; projectIds?: string[] }) =>
    req<GroupDetail>('PATCH', `/groups/${id}`, patch),
  deleteGroup: (id: string) => req<{ ok: true }>('DELETE', `/groups/${id}`),
  groupContext: (id: string) => req<{ context: string }>('GET', `/groups/${id}/context`),
  projectFiles: (id: string) => req<{ files: string[] }>('GET', `/projects/${id}/files`),
  /** One harness (a tab of the Harness page). */
  harnessOne: (kind: AgentKind, refresh = false) => req<{ info: HarnessInfo; checkedAt: string }>('GET', `/harness/${kind}${refresh ? '?refresh=1' : ''}`),
  agentModels: (kind: AgentKind) => req<AgentModelsResponse>('GET', `/agents/${kind}/models`),
  harnessAction: (kind: AgentKind, action: string, body: { cols?: number; rows?: number } = {}) =>
    req<JobInfo | { ok: true; output: string }>('POST', `/harness/${kind}/actions/${action}`, body),
  /** Provider credential of an agent with accountMode: providers. */
  harnessAccount: (kind: AgentKind, provider: string, account: string, op: 'switch' | 'logout') =>
    req<{ ok: true; output: string }>('POST', `/harness/${kind}/accounts/${encodeURIComponent(provider)}/${encodeURIComponent(account)}/${op}`),
  harnessDefaultModel: (kind: AgentKind, model: string | null) => req<{ ok: true }>('PUT', `/harness/${kind}/default-model`, { model }),
  /** MCP servers × agents, from the agents' own configs (with a project: its servers too). */
  mcp: (project?: string) => req<McpResponse>('GET', `/harness/mcp${q({ project })}`),
  mcpAdd: (input: McpServerInput, project?: string) => req<McpResponse>('POST', `/harness/mcp${q({ project })}`, input),
  mcpConnect: (name: string, kind: AgentKind, project?: string) =>
    req<McpResponse>('PUT', `/harness/mcp/${encodeURIComponent(name)}/agents/${kind}${q({ project })}`),
  mcpDisconnect: (name: string, kind: AgentKind, project?: string) =>
    req<McpResponse>('DELETE', `/harness/mcp/${encodeURIComponent(name)}/agents/${kind}${q({ project })}`),
  mcpDelete: (name: string, project?: string) => req<McpResponse>('DELETE', `/harness/mcp/${encodeURIComponent(name)}${q({ project })}`),
  harnessExtensions: (kind: AgentKind, project?: string) => req<HarnessExtensions>('GET', `/harness/${kind}/extensions${q({ project })}`),
  rulesFile: (kind: AgentKind, path: string, project?: string) => req<RulesFileContent>('GET', `/harness/${kind}/rules${q({ path, project })}`),
  saveRulesFile: (kind: AgentKind, path: string, content: string, project?: string) =>
    req<RulesFileContent>('PUT', `/harness/${kind}/rules${q({ path, project })}`, { content }),
  extensionFile: (kind: AgentKind, path: string, project?: string) => req<RulesFileContent>('GET', `/harness/${kind}/file${q({ path, project })}`),
  extensionsAll: (project?: string) => req<HarnessExtensionsAll>('GET', `/harness/extensions${q({ project })}`),
  mcpCatalog: (query: string, cursor?: string) => req<McpCatalogResponse>('GET', `/harness/mcp/catalog${q({ q: query || undefined, cursor })}`),
  skills: (project?: string) => req<SkillsResponse>('GET', `/harness/skills${q({ project })}`),
  skill: (name: string, agent?: AgentKind, project?: string) => req<SkillContent>('GET', `/harness/skills/${encodeURIComponent(name)}${q({ agent, project })}`),
  skillConnect: (name: string, kind: AgentKind, project?: string) =>
    req<SkillsResponse>('PUT', `/harness/skills/${encodeURIComponent(name)}/agents/${kind}${q({ project })}`),
  skillDisconnect: (name: string, kind: AgentKind, project?: string) =>
    req<SkillsResponse>('DELETE', `/harness/skills/${encodeURIComponent(name)}/agents/${kind}${q({ project })}`),
  skillDelete: (name: string, project?: string) => req<SkillsResponse>('DELETE', `/harness/skills/${encodeURIComponent(name)}${q({ project })}`),
  skillCreate: (input: SkillCreateInput, project?: string) => req<SkillsResponse>('POST', `/harness/skills${q({ project })}`, input),
  skillImportPreview: (url: string) => req<SkillImportPreview>('POST', '/harness/skills/import/preview', { url }),
  skillImport: (input: SkillImportInput, project?: string) => req<SkillsResponse>('POST', `/harness/skills/import${q({ project })}`, input),
  jobInput: (id: string, data: string) => req<{ ok: true }>('POST', `/jobs/${id}/input`, { data }),
  jobKill: (id: string) => req<{ ok: boolean }>('POST', `/jobs/${id}/kill`),
  browse: (path?: string) => req<BrowseResponse>('GET', `/fs/browse${q({ path })}`),

  projects: () => req<Project[]>('GET', '/projects'),
  project: (id: string) => req<ProjectDetail>('GET', `/projects/${id}`),
  updateProject: (id: string, patch: { name?: string; description?: string; instructions?: string }) => req<Project>('PATCH', `/projects/${id}`, patch),

  sessions: (scope: { projectId?: string; groupId?: string } = {}) => req<Session[]>('GET', `/sessions${q(scope)}`),
  session: (id: string) => req<Session>('GET', `/sessions/${id}`),
  allSessions: () => req<Session[]>('GET', '/sessions?all=1'),
  archivedSessions: () => req<Session[]>('GET', '/sessions?archived=1'),
  archiveSession: (id: string) => req<Session>('POST', `/sessions/${id}/archive`),
  restoreSession: (id: string) => req<Session>('POST', `/sessions/${id}/restore`),
  emptyArchive: () => req<{ ok: true; deleted: number }>('DELETE', '/sessions/archive'),
  createSession: (input: { projectId?: string | null; groupId?: string | null; agent: AgentConfig; prompt?: string; title?: string }) =>
    req<Session>('POST', '/sessions', input),
  /** Raw upload (not JSON): bytes in the body, the name in X-Filename. */
  uploadAttachment: async (sessionId: string, file: File): Promise<Attachment> => {
    const res = await fetch(`/api/sessions/${sessionId}/attachments`, {
      method: 'POST',
      headers: { 'X-Hubness-Request': '1', 'Content-Type': file.type || 'application/octet-stream', 'X-Filename': encodeURIComponent(file.name) },
      body: file,
    });
    const data = await res.json().catch(() => null);
    if (!res.ok) throw new ApiError(res.status, data?.error ?? t('api.error', { status: res.status }));
    return data as Attachment;
  },
  updateSession: (id: string, patch: { title?: string; agent?: Partial<AgentConfig> }) =>
    req<Session>('PATCH', `/sessions/${id}`, patch),
  deleteSession: (id: string) => req('DELETE', `/sessions/${id}`),
  send: (id: string, text: string, attachments?: Attachment[]) => req<Session>('POST', `/sessions/${id}/messages`, { text, attachments }),
  stop: (id: string) => req('POST', `/sessions/${id}/stop`),
  approve: (id: string, requestId: string, d: ApprovalDecision) =>
    req('POST', `/sessions/${id}/approvals/${requestId}`, d),

  tree: (projectId: string, path = '') => req<FsEntry[]>('GET', `/projects/${projectId}/tree${q({ path })}`),
  file: (projectId: string, path: string) => req<FileContent>('GET', `/projects/${projectId}/file${q({ path })}`),
  search: (projectId: string, o: SearchOptions) =>
    req<SearchResult>(
      'GET',
      `/projects/${projectId}/search${q({ q: o.q, regex: o.regex ? '1' : undefined, case: o.caseSensitive ? '1' : undefined, word: o.word ? '1' : undefined, include: o.include || undefined, exclude: o.exclude || undefined })}`,
    ),
  fsCreate: (projectId: string, path: string, dir: boolean) => req<{ path: string }>('POST', `/projects/${projectId}/fs/create`, { path, dir }),
  fsRename: (projectId: string, from: string, to: string) => req<{ path: string }>('POST', `/projects/${projectId}/fs/rename`, { from, to }),
  fsDelete: (projectId: string, path: string) => req<{ ok: true }>('POST', `/projects/${projectId}/fs/delete`, { path }),
  saveFile: (projectId: string, body: FileWrite) => req<FileContent>('PUT', `/projects/${projectId}/file`, body),
  projectSuggestion: (sessionId: string) => req<ProjectSuggestion>('GET', `/sessions/${sessionId}/project-suggestion`),
  convertSession: (sessionId: string, name: string) =>
    req<{ project: Project; session: Session }>('POST', `/sessions/${sessionId}/convert`, { name }),
  paneFile: (sessionId: string, path: string) => req<FileContent>('GET', `/sessions/${sessionId}/file${q({ path })}`),
  paneDiff: (sessionId: string, path: string) => req<{ diff: string }>('GET', `/sessions/${sessionId}/diff${q({ path })}`),
  closePane: (sessionId: string) => req('POST', `/sessions/${sessionId}/pane`, { pane: null }),

  sysPlatform: () => req<SystemPlatform>('GET', '/system/platform'),
  sysOverview: () => req<SystemOverview>('GET', '/system/overview'),
  containers: () => req<{ available: boolean; containers: ContainerInfo[] }>('GET', '/system/containers'),
  containerAction: (name: string, action: string) => req<GitOpResult>('POST', `/system/containers/${encodeURIComponent(name)}/${action}`),
  containerRemove: (name: string) => req<GitOpResult>('DELETE', `/system/containers/${encodeURIComponent(name)}`),
  networks: () => req<{ available: boolean; networks: DockerNetwork[] }>('GET', '/system/networks'),
  networkRemove: (name: string) => req<GitOpResult>('DELETE', `/system/networks/${encodeURIComponent(name)}`),
  volumes: () => req<{ available: boolean; volumes: DockerVolume[] }>('GET', '/system/volumes'),
  volumeRemove: (name: string) => req<GitOpResult>('DELETE', `/system/volumes/${encodeURIComponent(name)}`),
  containerLogs: (name: string) => req<{ logs: string }>('GET', `/system/containers/${encodeURIComponent(name)}/logs`),
  units: () => req<UnitInfo[]>('GET', '/system/units'),
  unitAction: (scope: string, name: string, action: string) =>
    req<GitOpResult>('POST', `/system/units/${scope}/${encodeURIComponent(name)}/${action}`),
  unitLogs: (scope: string, name: string) => req<{ logs: string }>('GET', `/system/units/${scope}/${encodeURIComponent(name)}/logs`),
  packageManagers: () => req<{ managers: PackageManagerInfo[] }>('GET', '/system/packages'),
  packages: (manager: string, refresh = false) => req<PackagesResponse>('GET', `/system/packages/${manager}${refresh ? '?refresh=1' : ''}`),


  gitStatus: (projectId: string) => req<GitStatus>('GET', `/projects/${projectId}/git/status`),
  gitDiff: (projectId: string, opts: { path?: string; staged?: boolean; untracked?: boolean } = {}) =>
    req<{ diff: string }>(
      'GET',
      `/projects/${projectId}/git/diff${q({
        path: opts.path,
        staged: opts.staged ? '1' : undefined,
        untracked: opts.untracked ? '1' : undefined,
      })}`,
    ),
  gitBranches: (projectId: string) => req<GitBranch[]>('GET', `/projects/${projectId}/git/branches`),
  gitLog: (projectId: string, opts: { limit?: number; all?: boolean } = {}) =>
    req<GitLogResponse>('GET', `/projects/${projectId}/git/log?limit=${opts.limit ?? 150}&all=${opts.all === false ? 0 : 1}`),
  gitShow: (projectId: string, rev: string) => req<{ diff: string }>('GET', `/projects/${projectId}/git/show?rev=${encodeURIComponent(rev)}`),
  gitStashes: (projectId: string) => req<GitStash[]>('GET', `/projects/${projectId}/git/stashes`),
  gitOp: (projectId: string, op: string, body?: Record<string, unknown>) =>
    req<GitOpResult>('POST', `/projects/${projectId}/git/${op}`, body ?? {}),
  gitCredentials: () => req<GitCredentialInfo[]>('GET', '/git-credentials'),
  setGitCredential: (host: string, body: { username: string; kind: GitCredentialKind; secret: string }) =>
    req<GitCredentialInfo[]>('PUT', `/git-credentials/${encodeURIComponent(host)}`, body),
  deleteGitCredential: (host: string) => req<GitCredentialInfo[]>('DELETE', `/git-credentials/${encodeURIComponent(host)}`),
  users: () => req<User[]>('GET', '/users'),
  createUser: (body: { username: string; password: string }) => req<User>('POST', '/users', body),
  deleteUser: (id: string) => req<{ ok: true }>('DELETE', `/users/${encodeURIComponent(id)}`),
  /** Your own password needs `currentPassword`; ends the account's other sessions. */
  setUserPassword: (id: string, body: { password: string; currentPassword?: string }) =>
    req<{ ok: true }>('PUT', `/users/${encodeURIComponent(id)}/password`, body),
};
