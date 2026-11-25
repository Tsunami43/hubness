import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { homedir, userInfo } from 'node:os';
import { join } from 'node:path';
import type { EnvironmentStatus, Project, ProjectGroup, ProjectRuntime } from '@hubness/shared';
import { config, repoRoot } from './config.ts';
import { groups, projectRuntime, projects } from './db/index.ts';
import { HttpError } from './errors.ts';
import { shq } from './terminal.ts';
import { AGENTS } from './agents/registry.ts';
import { sh } from './system/exec.ts';

/**
 * Project execution environment — where and how Run, Shell and the agent run.
 *
 * - **local** — processes on the server, in the repository folder.
 * - **sandbox** (bubblewrap) — the same processes, but sandboxed: the system is
 *   read-only; the projects folder, other projects, hub data, keys (~/.ssh,
 *   ~/.aws…) and docker.sock are hidden; only the scope's repositories and agent
 *   state folders are writable. The network is shared (agents need the model API and
 *   the hub MCP). This closes off "the agent peeked into a neighboring repository".
 * - **docker** — Run and Shell in the project container (modeled on Coder: the container
 *   is disposable, home is a persistent folder; not root; no docker.sock; labels with the
 *   project id). The agent stays on the server (CLI and login live there), sandboxed.
 */

const HOME = homedir();
const DEFAULT_IMAGE = 'node:24-bookworm';

// ---------- availability ----------

let dockerOk: boolean | null = null;
let bwrapOk: boolean | null = null;

export function dockerAvailable(): boolean {
  if (dockerOk === null) dockerOk = spawnSync('docker', ['version', '--format', '{{.Server.Version}}'], { timeout: 5000 }).status === 0;
  return dockerOk;
}

export function sandboxAvailable(): boolean {
  if (bwrapOk === null) bwrapOk = spawnSync('bwrap', ['--ro-bind', '/', '/', '--dev', '/dev', 'true'], { timeout: 5000 }).status === 0;
  return bwrapOk;
}

// ---------- project settings ----------

/** JSONC → JSON: comments and trailing commas outside strings (// inside "https://…" is left alone). */
export function stripJsonc(text: string): string {
  let out = '';
  for (let i = 0; i < text.length; i++) {
    const c = text[i]!;
    if (c === '"') {
      let j = i + 1;
      while (j < text.length && text[j] !== '"') j += text[j] === '\\' ? 2 : 1;
      out += text.slice(i, j + 1);
      i = j;
    } else if (c === '/' && text[i + 1] === '/') {
      while (i < text.length && text[i] !== '\n') i++;
      out += '\n';
    } else if (c === '/' && text[i + 1] === '*') {
      const end = text.indexOf('*/', i + 2);
      i = end < 0 ? text.length : end + 1;
    } else out += c;
  }
  return out.replace(/,(\s*[}\]])/g, '$1');
}

/** Image from .devcontainer/devcontainer.json (image field only; Dockerfile builds are not supported). */
export function devcontainerImage(dir: string): string | null {
  for (const f of ['.devcontainer/devcontainer.json', '.devcontainer.json']) {
    const p = join(dir, f);
    if (!existsSync(p)) continue;
    try {
      const image = (JSON.parse(stripJsonc(readFileSync(p, 'utf8'))) as { image?: unknown }).image;
      if (typeof image === 'string' && image.trim()) return image.trim();
    } catch {}
  }
  return null;
}

export function runtimeOf(p: Project): ProjectRuntime {
  const saved = projectRuntime.get(p.id);
  return {
    environment: saved.environment === 'docker' ? 'docker' : 'local',
    image: saved.image?.trim() || devcontainerImage(p.path) || DEFAULT_IMAGE,
    // the sandbox is on by default if bubblewrap is available
    sandbox: saved.sandbox ?? sandboxAvailable(),
    shareSecretsWithAgents: saved.shareSecretsWithAgents ?? false,
  };
}

const IMAGE_RE = /^[a-z0-9][a-z0-9._/:@-]{0,254}$/i;

/** Only explicitly set fields are saved — defaults (the devcontainer image) stay live. */
export function setRuntime(p: Project, b: Partial<Record<keyof ProjectRuntime, unknown>>): ProjectRuntime {
  const saved = { ...projectRuntime.get(p.id) };
  if (b.environment !== undefined) {
    if (b.environment !== 'local' && b.environment !== 'docker') throw new HttpError(400, 'environment must be local or docker');
    if (b.environment === 'docker' && !dockerAvailable()) throw new HttpError(400, 'Docker is not available on this server');
    saved.environment = b.environment;
  }
  if (b.image !== undefined) {
    if (typeof b.image !== 'string' || (b.image.trim() && !IMAGE_RE.test(b.image.trim()))) throw new HttpError(400, 'Invalid image name');
    const image = b.image.trim();
    // empty or equal to the default image — don't pin it
    if (!image || image === (devcontainerImage(p.path) || DEFAULT_IMAGE)) delete saved.image;
    else saved.image = image;
  }
  if (b.sandbox !== undefined) {
    if (typeof b.sandbox !== 'boolean') throw new HttpError(400, 'sandbox must be a boolean');
    if (b.sandbox && !sandboxAvailable()) throw new HttpError(400, 'bubblewrap (bwrap) is not installed on this server');
    saved.sandbox = b.sandbox;
  }
  if (b.shareSecretsWithAgents !== undefined) {
    if (typeof b.shareSecretsWithAgents !== 'boolean') throw new HttpError(400, 'shareSecretsWithAgents must be a boolean');
    saved.shareSecretsWithAgents = b.shareSecretsWithAgents;
  }
  projectRuntime.set(p.id, saved);
  return runtimeOf(p);
}

// ---------- sandbox (bubblewrap) ----------

/** What to hide: other projects, hub data, keys and credentials, the Docker socket. */
function hiddenPaths(scopeRepos: string[]): string[] {
  const inScope = (p: string) => scopeRepos.some((r) => r === p || r.startsWith(p + '/') || p.startsWith(r + '/'));
  const list = [
    config.projectsDir,
    ...groups.list().map((g) => g.folder),
    ...projects.list().filter((p) => p.kind === 'normal' && !inScope(p.path)).map((p) => p.path),
    config.dataDir,
    config.hubHome,
    join(repoRoot, '.env'),
    ...['.ssh', '.gnupg', '.aws', '.azure', '.kube', '.docker', '.password-store', '.netrc', '.git-credentials', '.mozilla', '.pki'].map((d) => join(HOME, d)),
    ...['.config/google-chrome', '.config/chromium', '.config/BraveSoftware', '.config/gh', '.config/gcloud', '.local/share/keyrings'].map((d) => join(HOME, d)),
    '/var/run/docker.sock',
    '/run/docker.sock',
    `/run/user/${userInfo().uid}`,
  ];
  const existing = [...new Set(list.filter((p): p is string => Boolean(p) && existsSync(p!)))];
  // don't mount folders nested in an already hidden one: the mount point would reveal another project's name
  return existing.filter((p) => !existing.some((q) => q !== p && p.startsWith(q + '/')));
}

/** Agent state folders — writable (login, history, cache). */
const agentState = () => [...['.cache', '.npm', '.bun'].map((d) => join(HOME, d)), ...AGENTS.flatMap((a) => a.statePaths())];

/**
 * bwrap arguments for a scope (project or group).
 * rw — the scope's repositories and cwd; everything else is read-only.
 */
export function bwrapArgs(opts: { repos: string[]; cwd: string; interactive?: boolean }): string[] {
  // own PID namespace: when the agent/command exits, no processes are left inside
  const args = ['--ro-bind', '/', '/', '--dev', '/dev', '--unshare-pid', '--proc', '/proc', '--tmpfs', '/tmp', '--die-with-parent'];
  // without a controlling terminal the agent can't inject input into another tty (TIOCSTI)
  if (!opts.interactive) args.push('--new-session');
  for (const p of hiddenPaths(opts.repos)) {
    if (statSync(p).isDirectory()) args.push('--tmpfs', p);
    else args.push('--ro-bind', '/dev/null', p);
  }
  // the agent's MCP server needs the hub code (hub-mcp.ts); .data and .env are already hidden above
  if (existsSync(repoRoot)) {
    args.push('--ro-bind', repoRoot, repoRoot);
    for (const p of [config.dataDir, join(repoRoot, '.env')]) {
      if (!existsSync(p) || !p.startsWith(repoRoot + '/')) continue;
      if (statSync(p).isDirectory()) args.push('--tmpfs', p);
      else args.push('--ro-bind', '/dev/null', p);
    }
  }
  for (const p of agentState()) if (existsSync(p)) args.push('--bind', p, p);
  // interactive shell history is separate (the host one with typed commands is not visible)
  if (opts.interactive) {
    const hist = join(config.dataDir, 'shell-history');
    // bound below as is (HISTFILE lives there): it must exist even without fish/zsh folders
    mkdirSync(hist, { recursive: true });
    for (const [dir, target] of [
      [join(hist, 'fish'), join(HOME, '.local/share/fish')],
      [join(hist, 'zsh'), join(HOME, '.local/share/zsh')],
    ] as const) {
      // a mount point can't be created on a read-only system — only existing folders
      if (!existsSync(target)) continue;
      mkdirSync(dir, { recursive: true });
      args.push('--bind', dir, target);
    }
    args.push('--bind', hist, hist, '--setenv', 'HISTFILE', join(hist, 'bash_history'));
  }
  for (const r of opts.repos) args.push('--bind', r, r);
  if (!opts.repos.includes(opts.cwd)) args.push('--bind', opts.cwd, opts.cwd);
  args.push('--chdir', opts.cwd);
  return args;
}

// ---------- docker ----------

export const containerName = (projectId: string) => `hubness-p-${projectId.slice(0, 12)}`;
const containerHome = (projectId: string) => join(config.dataDir, 'containers', projectId, 'home');

export async function containerState(projectId: string): Promise<{ state: 'running' | 'stopped' | 'absent'; image: string | null }> {
  if (!dockerAvailable()) return { state: 'absent', image: null };
  const r = await sh('docker', ['inspect', '--format', '{{.State.Running}} {{.Config.Image}}', containerName(projectId)], 5000);
  if (r.code !== 0) return { state: 'absent', image: null };
  const [running, image] = r.out.trim().split(' ');
  return { state: running === 'true' ? 'running' : 'stopped', image: image ?? null };
}

/** Recent container IPs: the preview proxy asks on every request (a page load is hundreds). */
const ipCache = new Map<string, { at: number; ip: Promise<string | null> }>();
const IP_TTL_MS = 5000;

export function containerIp(projectId: string): Promise<string | null> {
  const hit = ipCache.get(projectId);
  if (hit && Date.now() - hit.at < IP_TTL_MS) return hit.ip;
  const ip = sh('docker', ['inspect', '--format', '{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}', containerName(projectId)], 5000).then((r) =>
    r.code === 0 ? r.out.trim().split(/\s+/)[0] || null : null,
  );
  ipCache.set(projectId, { at: Date.now(), ip });
  return ip;
}

/**
 * sh script: bring up the project container if it doesn't exist or the image changed.
 * Runs inside a terminal task — the image pull is visible in the output.
 */
function ensureContainerScript(p: Project, rt: ProjectRuntime): string {
  const name = containerName(p.id);
  const home = containerHome(p.id);
  mkdirSync(home, { recursive: true });
  const { uid, gid } = userInfo();
  const run = [
    'docker run -d --init',
    `--name ${shq(name)}`,
    `--label hubness.project=${shq(p.id)}`,
    `--user ${uid}:${gid}`,
    `-e HOME=/home/hub`,
    `-v ${shq(home)}:/home/hub`,
    `-v ${shq(p.path)}:/workspace`,
    '-w /workspace',
    '--add-host host.docker.internal:host-gateway',
    shq(rt.image),
    'sleep infinity',
  ].join(' ');
  return [
    `cur=$(docker inspect --format '{{.Config.Image}}' ${shq(name)} 2>/dev/null || true)`,
    `if [ -n "$cur" ] && [ "$cur" != ${shq(rt.image)} ]; then echo "[hub] image changed: recreating container"; docker rm -f ${shq(name)} >/dev/null; cur=; fi`,
    `if [ -z "$cur" ]; then echo "[hub] starting container ${name} (${rt.image})"; ${run} >/dev/null || exit 1; else docker start ${shq(name)} >/dev/null || exit 1; fi`,
  ].join('\n');
}

export async function stopContainer(projectId: string): Promise<void> {
  ipCache.delete(projectId);
  if (dockerAvailable()) await sh('docker', ['rm', '-f', containerName(projectId)], 30_000);
}

// ---------- commands for terminal tasks ----------

/** Environment file (secrets) for docker exec: don't expose values in `ps`. */
function envFile(projectId: string, env: Record<string, string>): string {
  const dir = join(config.dataDir, 'containers', projectId);
  mkdirSync(dir, { recursive: true });
  const f = join(dir, 'exec.env');
  writeFileSync(f, Object.entries(env).map(([k, v]) => `${k}=${v.replace(/\n/g, '\\n')}`).join('\n') + '\n', { mode: 0o600 });
  return f;
}

export type Scope = { kind: 'project'; project: Project } | { kind: 'group'; group: ProjectGroup; projects: Project[]; cwd: string };

const scopeRepos = (s: Scope) => (s.kind === 'project' ? [s.project.path] : s.projects.map((p) => p.path));
const scopeCwd = (s: Scope) => (s.kind === 'project' ? s.project.path : s.cwd);
/** A group is sandboxed if bubblewrap is available (a group has no environment settings of its own). */
// the system project (home, questions about the server) runs without a sandbox
const scopeSandbox = (s: Scope) => (s.kind === 'project' ? s.project.kind === 'normal' && runtimeOf(s.project).sandbox : sandboxAvailable());

const userShell = () => {
  const sh = userInfo().shell || process.env.SHELL || '/bin/bash';
  return existsSync(sh) ? sh : '/bin/sh';
};

/**
 * Command line for a terminal task (run by bash inside a pty).
 * command = null — interactive shell.
 */
export function execCommand(
  scope: Scope,
  command: string | null,
  env: Record<string, string>,
  opts: { noSandbox?: boolean } = {},
): { command: string; cwd: string; env: Record<string, string>; label: string; onKill?: () => void } {
  const cwd = scopeCwd(scope);
  if (scope.kind === 'project' && runtimeOf(scope.project).environment === 'docker') {
    const rt = runtimeOf(scope.project);
    const name = containerName(scope.project.id);
    const body = command ?? 'if command -v bash >/dev/null; then exec bash -l; else exec sh -l; fi';
    // the process in the container doesn't die with the docker exec client — remember
    // its group (with -t it leads its own session) and kill it on stop
    const tag = `/tmp/hubness-${Math.random().toString(36).slice(2, 10)}.pid`;
    const inner = `echo $$ > ${tag}; ${body}`;
    // preview connects to the container IP — the app must listen on 0.0.0.0 (many dev servers honor HOST)
    const exec = `docker exec -it --env-file ${shq(envFile(scope.project.id, { HOST: '0.0.0.0', ...env }))} -w /workspace ${shq(name)} sh -lc ${shq(inner)}`;
    return {
      command: `${ensureContainerScript(scope.project, rt)}\n${exec}`,
      cwd,
      env: {},
      label: `${command ?? 'shell'} · docker ${rt.image}`,
      onKill: () => {
        spawn('docker', ['exec', name, 'sh', '-c', `kill -TERM -$(cat ${tag}) 2>/dev/null; sleep 2; kill -KILL -$(cat ${tag}) 2>/dev/null; rm -f ${tag}`], {
          stdio: 'ignore',
        }).unref();
      },
    };
  }
  const inner = command === null ? `exec ${shq(userShell())} -l` : `exec bash -lc ${shq(command)}`;
  if (scopeSandbox(scope) && !opts.noSandbox) {
    const args = bwrapArgs({ repos: scopeRepos(scope), cwd, interactive: true });
    return { command: `exec bwrap ${args.map(shq).join(' ')} -- bash -c ${shq(inner)}`, cwd, env, label: `${command ?? userShell()} · sandbox` };
  }
  return { command: inner, cwd, env, label: command ?? userShell() };
}

/** Agent launch wrapper: sandbox for project/group sessions. */
export function agentWrap(scope: Scope | null): ((cmd: string, args: string[]) => [string, string[]]) | null {
  if (!scope || !scopeSandbox(scope)) return null;
  const repos = scopeRepos(scope);
  const cwd = scopeCwd(scope);
  return (cmd, args) => ['bwrap', [...bwrapArgs({ repos, cwd }), '--', cmd, ...args]];
}

export async function environmentStatus(p: Project): Promise<EnvironmentStatus> {
  const rt = runtimeOf(p);
  return {
    runtime: rt,
    dockerAvailable: dockerAvailable(),
    sandboxAvailable: sandboxAvailable(),
    container: rt.environment === 'docker' ? { name: containerName(p.id), ...(await containerState(p.id)) } : null,
    devcontainerImage: devcontainerImage(p.path),
  };
}
