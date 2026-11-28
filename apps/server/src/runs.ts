import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { JobInfo, Project, RunConfig, RunState, RunSuggestion } from '@hubness/shared';
import { runConfigs } from './db/index.ts';
import { HttpError } from './errors.ts';
import { containerIp, execCommand, runtimeOf, type Scope } from './environments.ts';
import { closePreview, closePreviewsWithPrefix, ensurePreview, previewPort } from './preview.ts';
import { secretEnv } from './secrets.ts';
import { exposePreview } from './tailscale.ts';
import { getJob, jobListeners, killJob, startJob } from './terminal.ts';

/**
 * Run (like Run/Workflows in Replit): named project commands, each with
 * one run at a time, output in the hub terminal, app port → preview.
 * Shell — an interactive shell of a project or group; lives until it is
 * closed or nobody watches it for ~30 minutes; reconnect by task id.
 * Processes run in the project environment (environments.ts): on the server, sandboxed or in
 * a container; project secrets are in the environment.
 */

// ---------- commands ----------

const NAME_MAX = 60;
const COMMAND_MAX = 2000;

function validate(b: { name?: unknown; command?: unknown; port?: unknown; docker?: unknown }, partial: boolean) {
  const out: { name?: string; command?: string; port?: number | null; docker?: boolean } = {};
  if (b.docker !== undefined) {
    if (typeof b.docker !== 'boolean') throw new HttpError(400, 'docker must be a boolean');
    out.docker = b.docker;
  }
  if (b.name !== undefined || !partial) {
    if (typeof b.name !== 'string' || !b.name.trim() || b.name.length > NAME_MAX) throw new HttpError(400, `Name is required (max ${NAME_MAX})`);
    out.name = b.name.trim();
  }
  if (b.command !== undefined || !partial) {
    if (typeof b.command !== 'string' || !b.command.trim() || b.command.length > COMMAND_MAX) throw new HttpError(400, 'Command is required');
    out.command = b.command.trim();
  }
  if (b.port !== undefined) {
    if (b.port === null || b.port === '') out.port = null;
    else {
      const n = Number(b.port);
      if (!Number.isInteger(n) || n < 1 || n > 65535) throw new HttpError(400, 'Port must be 1–65535');
      out.port = n;
    }
  }
  return out;
}

export function createRun(p: Project, b: { name?: unknown; command?: unknown; port?: unknown; primary?: unknown; docker?: unknown }): RunState {
  const v = validate(b, false);
  return state(p, runConfigs.create(p.id, { name: v.name!, command: v.command!, port: v.port ?? null, primary: b.primary === true, docker: v.docker }));
}

export function updateRun(p: Project, id: string, b: { name?: unknown; command?: unknown; port?: unknown; primary?: unknown; docker?: unknown }): RunState {
  const cur = runOr404(p, id);
  runConfigs.update(id, validate(b, true));
  if (b.primary === true) runConfigs.setPrimary(p.id, id);
  return state(p, runConfigs.get(cur.id)!);
}

export function deleteRun(p: Project, id: string) {
  runOr404(p, id);
  stopRun(p, id);
  runConfigs.delete(id);
}

function runOr404(p: Project, id: string): RunConfig {
  const r = runConfigs.get(id);
  if (!r || r.projectId !== p.id) throw new HttpError(404, 'Run command not found');
  return r;
}

// ---------- suggestions ----------

function packageManager(dir: string): string {
  if (existsSync(join(dir, 'pnpm-lock.yaml'))) return 'pnpm';
  if (existsSync(join(dir, 'yarn.lock'))) return 'yarn';
  if (existsSync(join(dir, 'bun.lockb')) || existsSync(join(dir, 'bun.lock'))) return 'bun';
  return 'npm';
}

/** How to run Python: the project environment (uv/poetry/pipenv), else the system python. */
function pythonRunner(dir: string): string {
  if (existsSync(join(dir, 'uv.lock'))) return 'uv run ';
  if (existsSync(join(dir, 'poetry.lock'))) return 'poetry run ';
  if (existsSync(join(dir, 'Pipfile.lock'))) return 'pipenv run ';
  return '';
}

const read = (f: string) => {
  try {
    return readFileSync(f, 'utf8');
  } catch {
    return '';
  }
};

/** FastAPI/Flask app file: look for `FastAPI(` / `Flask(` in typical places. */
function findApp(dir: string, marker: RegExp): { file: string; module: string } | null {
  for (const f of ['main.py', 'app.py', 'server.py', 'app/main.py', 'src/main.py', 'api/main.py']) {
    if (marker.test(read(join(dir, f)))) return { file: f, module: f.replace(/\.py$/, '').replace(/\//g, '.') };
  }
  return null;
}

/**
 * Commands from the repository. Python goes through the project environment (`uv run …`),
 * otherwise `python main.py` doesn't see dependencies (No module named …). Web frameworks get
 * a dev server with a port right away (for preview). compose gets the Docker flag.
 */
export function suggestRuns(p: Project): RunSuggestion[] {
  const dir = p.path;
  const out: RunSuggestion[] = [];
  const pkgFile = join(dir, 'package.json');
  if (existsSync(pkgFile)) {
    try {
      const scripts = (JSON.parse(readFileSync(pkgFile, 'utf8')) as { scripts?: Record<string, string> }).scripts ?? {};
      const pm = packageManager(dir);
      const order = ['dev', 'start', 'serve', 'preview', 'test', 'build', 'lint'];
      const names = Object.keys(scripts).sort((a, b) => (order.indexOf(a) + 1 || 99) - (order.indexOf(b) + 1 || 99));
      for (const n of names.slice(0, 12)) out.push({ name: n, command: pm === 'npm' && n !== 'start' && n !== 'test' ? `npm run ${n}` : `${pm} ${n}`, source: 'package.json' });
    } catch {}
  }

  // Python
  const py = read(join(dir, 'pyproject.toml')) + '\n' + read(join(dir, 'requirements.txt'));
  const run = pythonRunner(dir);
  const has = (dep: string) => new RegExp(`["'\\s^]${dep}([\\[<>=~!"' ,]|$)`, 'im').test(py);
  const fastapi = has('fastapi') ? findApp(dir, /FastAPI\(/) : null;
  if (fastapi) {
    const cli = /fastapi\[standard\]|fastapi-cli/i.test(py);
    out.push({
      name: 'dev',
      command: cli ? `${run}fastapi dev ${fastapi.file}` : `${run}${run === 'uv run ' && !has('uvicorn') ? '--with uvicorn ' : ''}uvicorn ${fastapi.module}:app --reload`,
      source: 'FastAPI',
      port: 8000,
    });
  }
  const flask = has('flask') ? findApp(dir, /Flask\(/) : null;
  if (flask) out.push({ name: 'dev', command: `${run}flask --app ${flask.module} run --debug`, source: 'Flask', port: 5000 });
  if (existsSync(join(dir, 'manage.py'))) out.push({ name: 'django', command: `${run}python manage.py runserver`, source: 'manage.py', port: 8000 });
  // packages with __main__.py → python -m package
  const pkgs = readdirSafe(dir).filter((d) => !d.startsWith('.') && existsSync(join(dir, d, '__main__.py')));
  for (const d of pkgs.slice(0, 3)) out.push({ name: d, command: `${run}python -m ${d}`, source: `${d}/__main__.py` });
  const pyFiles = Boolean(py.trim()) || pkgs.length > 0 || readdirSafe(dir).some((f) => f.endsWith('.py'));
  const testsDir = readdirSafe(join(dir, 'tests')).some((f) => f.endsWith('.py'));
  if (pyFiles && (has('pytest') || testsDir || readdirSafe(dir).some((f) => /^test_.*\.py$/.test(f))))
    out.push({ name: 'test', command: has('pytest') ? `${run}pytest` : `${run}python -m unittest -v`, source: has('pytest') ? 'pytest' : 'unittest' });
  const scripts = /\[project\.scripts\]([\s\S]*?)(\n\[|$)/.exec(read(join(dir, 'pyproject.toml')))?.[1] ?? '';
  for (const m of scripts.matchAll(/^\s*([\w.-]+)\s*=/gm)) out.push({ name: m[1]!, command: `${run}${m[1]}`, source: 'pyproject.toml' });
  if (!fastapi && !flask)
    for (const f of ['main.py', 'app.py']) if (existsSync(join(dir, f))) out.push({ name: f.replace('.py', ''), command: `${run || 'python '}${run ? 'python ' : ''}${f}`, source: f });

  const make = join(dir, 'Makefile');
  if (existsSync(make)) {
    const targets = [...readFileSync(make, 'utf8').matchAll(/^([a-zA-Z][\w-]*):(?!=)/gm)].map((m) => m[1]!);
    for (const t of [...new Set(targets)].slice(0, 8)) out.push({ name: t, command: `make ${t}`, source: 'Makefile' });
  }
  for (const f of ['compose.yaml', 'compose.yml', 'docker-compose.yml', 'docker-compose.yaml'])
    if (existsSync(join(dir, f))) {
      out.push({ name: 'compose up', command: 'docker compose up --build', source: f, docker: true });
      out.push({ name: 'compose logs', command: 'docker compose logs -f --tail=200', source: f, docker: true });
      out.push({ name: 'compose down', command: 'docker compose down', source: f, docker: true });
      break;
    }
  if (existsSync(join(dir, 'go.mod'))) out.push({ name: 'go', command: 'go run .', source: 'go.mod' });
  if (existsSync(join(dir, 'Cargo.toml'))) out.push({ name: 'cargo', command: 'cargo run', source: 'Cargo.toml' });
  const known = new Set(runConfigs.list(p.id).map((r) => r.command));
  const seen = new Set<string>();
  return out.filter((s) => !known.has(s.command) && !seen.has(s.command) && seen.add(s.command));
}

function readdirSafe(dir: string): string[] {
  try {
    return readdirSync(dir);
  } catch {
    return [];
  }
}

// ---------- run ----------

interface Running {
  jobId: string;
  detectedPort: number | null;
  tail: string;
}
const running = new Map<string, Running>();

// http://localhost:5173, http://127.0.0.1:8000/, http://0.0.0.0:3000, http://[::1]:4000
const PORT_RE = /https?:\/\/(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\]|\d{1,3}(?:\.\d{1,3}){3})(?::(\d{2,5}))/;
const strip = (s: string) => s.replace(/\x1b\][^\x07]*(\x07|\x1b\\)/g, '').replace(/\x1b\[[0-9;?]*[a-zA-Z]/g, '');

const previewKey = (projectId: string, runId: string) => `${projectId}:${runId}`;

function previewTarget(p: Project, port: number) {
  return () => {
    if (runtimeOf(p).environment === 'docker') return containerIp(p.id).then((ip) => (ip ? { host: ip, port } : null));
    return { host: '127.0.0.1', port };
  };
}

function state(p: Project, r: RunConfig): RunState {
  const cur = running.get(r.id);
  const job = cur ? (getJob(cur.jobId)?.info ?? null) : null;
  const activePort = r.port ?? cur?.detectedPort ?? null;
  return { ...r, job, activePort, previewPort: job?.status === 'running' ? previewPort(previewKey(p.id, r.id)) : null };
}

export function listRuns(p: Project): RunState[] {
  return runConfigs.list(p.id).map((r) => state(p, r));
}

export async function startRun(p: Project, id: string, size: { cols?: number; rows?: number }): Promise<RunState> {
  const r = runOr404(p, id);
  const cur = running.get(id);
  if (cur && getJob(cur.jobId)?.info.status === 'running') return state(p, r);
  // Docker commands (compose) run without the sandbox: the Docker socket is hidden inside it
  const spec = execCommand({ kind: 'project', project: p }, r.command, secretEnv(p.id), { noSandbox: r.docker });
  const entry: Running = { jobId: '', detectedPort: null, tail: '' };
  const job = startJob({
    title: `${p.name} · ${r.name}`,
    command: spec.command,
    display: r.command,
    onKill: spec.onKill,
    cwd: spec.cwd,
    env: spec.env,
    cols: size.cols,
    rows: size.rows,
    onOutput: (data) => {
      if (entry.detectedPort || r.port) return;
      entry.tail = (entry.tail + strip(data)).slice(-4000);
      const m = PORT_RE.exec(entry.tail);
      if (m?.[1]) {
        entry.detectedPort = Number(m[1]);
        void ensurePreview(previewKey(p.id, r.id), previewTarget(p, entry.detectedPort));
      }
    },
    onExit: () => closePreview(previewKey(p.id, r.id)),
  });
  entry.jobId = job.id;
  running.set(id, entry);
  if (r.port) await ensurePreview(previewKey(p.id, r.id), previewTarget(p, r.port));
  return state(p, r);
}

export function stopRun(p: Project, id: string): RunState {
  const r = runOr404(p, id);
  const cur = running.get(id);
  if (cur) killJob(cur.jobId);
  closePreview(previewKey(p.id, id));
  return state(p, r);
}

/** Stop everything of a project (project removed from the hub, environment changed). */
export function stopProjectProcesses(projectId: string) {
  for (const r of runConfigs.list(projectId)) {
    const cur = running.get(r.id);
    if (cur) killJob(cur.jobId);
  }
  closePreviewsWithPrefix(`${projectId}:`);
  for (const sh of shells.get(`project:${projectId}`) ?? []) killJob(sh.jobId);
}

// ---------- shell ----------

// a scope can have several shells (tabs); each lives until it is
// closed or nobody watches it for ~30 minutes
const shells = new Map<string, { jobId: string; lastSeen: number }[]>();
const SHELL_IDLE_MS = 30 * 60_000;

setInterval(() => {
  const now = Date.now();
  for (const [key, list] of shells) {
    const alive = list.filter((s) => getJob(s.jobId)?.info.status === 'running');
    for (const s of alive) {
      if (jobListeners(s.jobId) > 0) s.lastSeen = now;
      else if (now - s.lastSeen > SHELL_IDLE_MS) killJob(s.jobId);
    }
    if (alive.length) shells.set(key, alive);
    else shells.delete(key);
  }
}, 60_000).unref();

const shellKey = (scope: Scope) => (scope.kind === 'project' ? `project:${scope.project.id}` : `group:${scope.group.id}`);

/** Open shells of a scope (for tabs, reconnecting after a page reload). */
export function listShells(scope: Scope): JobInfo[] {
  return (shells.get(shellKey(scope)) ?? []).map((s) => getJob(s.jobId)?.info).filter((i): i is JobInfo => i?.status === 'running');
}

/** Scope shell: fresh — one more (tab), else the first open one or a new one. Old ones are not closed. */
export function openShell(scope: Scope, size: { cols?: number; rows?: number }, fresh = false): JobInfo {
  const key = shellKey(scope);
  const open = listShells(scope);
  if (!fresh && open[0]) return open[0];
  const env = scope.kind === 'project' ? secretEnv(scope.project.id) : {};
  const spec = execCommand(scope, null, env);
  const n = open.length + 1;
  const job = startJob({
    title: `Shell ${n}`,
    command: spec.command,
    display: spec.label,
    onKill: spec.onKill,
    cwd: spec.cwd,
    env: spec.env,
    cols: size.cols,
    rows: size.rows,
  });
  shells.set(key, [...(shells.get(key) ?? []), { jobId: job.id, lastSeen: Date.now() }]);
  return job;
}

// ---------- preview of any port ----------

/** Preview of a port that something already listens on (started in Shell, docker compose…). */
export async function previewPortFor(p: Project, port: number): Promise<number> {
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new HttpError(400, 'Port must be 1–65535');
  // a running command on that port already has a preview: the same page (and log) as the Preview tab
  const run = listRuns(p).find((r) => r.activePort === port && r.previewPort);
  if (run?.previewPort) {
    // asked for again (the Preview tab's "Try again"): one more try at Tailscale Serve
    void exposePreview(run.previewPort);
    return run.previewPort;
  }
  const pp = await ensurePreview(`${p.id}:port:${port}`, previewTarget(p, port));
  if (!pp) throw new HttpError(503, 'No free preview ports (HUBNESS_PREVIEW_PORTS)');
  return pp;
}
