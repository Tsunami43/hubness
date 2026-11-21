import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  AppWindow,
  Box,
  ExternalLink,
  KeyRound,
  Pencil,
  Play,
  Plus,
  RotateCw,
  ShieldCheck,
  Square,
  Star,
  Terminal as TerminalIcon,
  Trash2,
  X,
} from 'lucide-react';
import type { EnvironmentStatus, JobInfo, RunState, RunSuggestion, SecretInfo } from '@hubness/shared';
import { api } from '../lib/api';
import { t } from '../i18n';
import { JobStatusLine, JobTerminal } from './Terminal';
import { Button, ErrorLine, Field, IconButton, inputCls, Spinner, Switch, Checkbox } from './ui';
import { Select } from './Select';

/**
 * pty size for the width where the terminal will appear: on desktop the tool panel
 * (its width is in localStorage), on phone the screen. xterm: ~7.9px per character.
 */
export function ptySize() {
  let w = window.innerWidth;
  if (window.matchMedia('(min-width: 1024px)').matches) {
    try {
      w = Number(JSON.parse(localStorage.getItem('hub.pane.width') ?? '520')) || 520;
    } catch {
      w = 520;
    }
  }
  return { cols: Math.max(40, Math.floor((w - 36) / 7.9)), rows: 30 };
}

export const previewUrl = (port: number, path = '/') => `${window.location.protocol}//${window.location.hostname}:${port}${path}`;

/** Project run commands; polled every 3 s while something is running or the panel is open. */
export function useRuns(projectId: string, active: boolean) {
  const [runs, setRuns] = useState<RunState[] | null>(null);
  const reload = useCallback(() => api.runs(projectId).then(setRuns, () => {}), [projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);
  const anyRunning = runs?.some((r) => r.job?.status === 'running') ?? false;
  useEffect(() => {
    if (!active && !anyRunning) return;
    const timer = setInterval(() => void reload(), 3000);
    return () => clearInterval(timer);
  }, [active, anyRunning, reload]);
  return { runs, reload, setRuns };
}

const isRunning = (r: RunState) => r.job?.status === 'running';

// ---------- Run ----------

/** Run button in the header: starts the main command; if it is running, Stop. */
export function RunButton({ runs, onRun, onStop, onOpen }: { runs: RunState[] | null; onRun: (r: RunState) => void; onStop: (r: RunState) => void; onOpen: () => void }) {
  const primary = runs?.find((r) => r.primary) ?? runs?.[0];
  if (!primary)
    return (
      <Button size="sm" onClick={onOpen} title={t('run.setup')}>
        <Play className="size-4" />
        <span className="hidden sm:inline">{t('run.run')}</span>
      </Button>
    );
  const on = isRunning(primary);
  return (
    <Button size="sm" variant={on ? 'danger' : 'secondary'} onClick={() => (on ? onStop(primary) : onRun(primary))} title={on ? t('run.stop') : `${t('run.run')}: ${primary.command}`}>
      {on ? <Square className="size-3.5" /> : <Play className="size-4 text-ok" />}
      <span className="hidden sm:inline">{on ? t('run.stop') : t('run.run')}</span>
    </Button>
  );
}

/**
 * Run tab: project commands. Each has visible actions: start/stop,
 * preview (when it listens on a port), main (★, the Run button in the header), edit,
 * delete. Below is the output of the selected command at full height. Suggestions from
 * the repository are added with one tap (with port and Docker flag).
 */
export function RunPanel({
  projectId,
  runs,
  reload,
  focus,
  setFocus,
  onPreview,
}: {
  projectId: string;
  runs: RunState[] | null;
  reload: () => void;
  focus: string | null;
  setFocus: (id: string | null) => void;
  onPreview: (r: RunState) => void;
}) {
  const [suggestions, setSuggestions] = useState<RunSuggestion[] | null>(null);
  const [editing, setEditing] = useState<string | 'new' | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setError(null);
    api.runSuggestions(projectId).then(setSuggestions, () => setSuggestions([]));
  }, [projectId, runs?.length]);
  useEffect(() => {
    if (!confirmDelete) return;
    const timer = setTimeout(() => setConfirmDelete(null), 3000);
    return () => clearTimeout(timer);
  }, [confirmDelete]);

  const act = async (fn: () => Promise<unknown>) => {
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError((e as Error).message);
    }
    reload();
  };
  const start = (r: RunState) =>
    act(async () => {
      await api.startRun(projectId, r.id, ptySize());
      setFocus(r.id);
    });

  const focused = runs?.find((r) => r.id === focus && r.job) ?? runs?.find((r) => isRunning(r)) ?? null;
  const iconBtn = 'inline-flex size-7 shrink-0 items-center justify-center text-muted hover:bg-sunken hover:text-fg';
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="max-h-[50%] shrink-0 space-y-2 overflow-y-auto border-b border-line p-2">
        {runs === null ? (
          <div className="flex justify-center py-6">
            <Spinner />
          </div>
        ) : null}
        {runs?.length ? (
          <ul className="divide-y divide-line border border-line">
            {runs.map((r) =>
              editing === r.id ? (
                <li key={r.id} className="p-2">
                  <RunForm
                    initial={r}
                    onCancel={() => setEditing(null)}
                    onSave={(v) =>
                      act(async () => {
                        await api.updateRun(projectId, r.id, v);
                        setEditing(null);
                      })
                    }
                  />
                </li>
              ) : (
                <li key={r.id} className={`flex items-center gap-1.5 py-1 pr-1 pl-2 ${focused?.id === r.id ? 'bg-surface' : ''}`}>
                  <span
                    title={isRunning(r) ? t('run.running') : r.job?.exitCode ? t('term.failed', { code: r.job.exitCode }) : ''}
                    className={`size-2 shrink-0 rounded-full ${isRunning(r) ? 'bg-ok' : r.job?.exitCode ? 'bg-danger' : 'bg-line'}`}
                  />
                  <button type="button" className="min-w-0 flex-1 py-0.5 text-left" onClick={() => setFocus(r.id)} title={t('run.logs')}>
                    <span className="flex items-center gap-1.5 text-[13px]">
                      <span className={focused?.id === r.id ? 'text-accent' : ''}>{r.name}</span>
                      {r.docker ? <span className="border border-line px-1 text-[10px] text-muted">docker</span> : null}
                      {r.activePort ? <span className="text-[11px] text-muted">:{r.activePort}</span> : null}
                    </span>
                    <span className="block truncate text-xs text-muted">{r.command}</span>
                  </button>
                  <button
                    type="button"
                    className={iconBtn}
                    title={r.primary ? t('run.isPrimary') : t('run.makePrimary')}
                    onClick={() => !r.primary && void act(() => api.updateRun(projectId, r.id, { primary: true }))}
                  >
                    <Star className={`size-3.5 ${r.primary ? 'fill-current text-warn' : ''}`} />
                  </button>
                  {isRunning(r) && r.previewPort ? (
                    <button type="button" className={iconBtn} title={t('preview.open')} onClick={() => onPreview(r)}>
                      <AppWindow className="size-3.5" />
                    </button>
                  ) : null}
                  <button type="button" className={iconBtn} title={t('run.edit')} onClick={() => setEditing(r.id)}>
                    <Pencil className="size-3.5" />
                  </button>
                  {confirmDelete === r.id ? (
                    <Button size="sm" variant="danger" onClick={() => act(() => api.deleteRun(projectId, r.id))}>
                      {t('run.deleteConfirm')}
                    </Button>
                  ) : (
                    <button type="button" className={`${iconBtn} hover:text-danger`} title={t('common.delete')} onClick={() => setConfirmDelete(r.id)}>
                      <Trash2 className="size-3.5" />
                    </button>
                  )}
                  {isRunning(r) ? (
                    <Button size="sm" variant="danger" onClick={() => act(() => api.stopRun(projectId, r.id))} title={t('run.stop')}>
                      <Square className="size-3 fill-current" />
                    </Button>
                  ) : (
                    <Button size="sm" onClick={() => void start(r)} title={t('run.run')}>
                      <Play className="size-3.5 text-ok" />
                    </Button>
                  )}
                </li>
              ),
            )}
          </ul>
        ) : null}

        {editing === 'new' ? (
          <RunForm
            onCancel={() => setEditing(null)}
            onSave={(v) =>
              act(async () => {
                await api.createRun(projectId, v);
                setEditing(null);
              })
            }
          />
        ) : (
          <Button size="sm" variant="ghost" onClick={() => setEditing('new')}>
            <Plus className="size-3.5" /> {t('run.add')}
          </Button>
        )}

        {suggestions?.length ? (
          <div>
            <div className="mb-1 text-xs text-muted">{runs?.length ? t('run.moreFromRepo') : t('run.fromRepo')}</div>
            <div className="flex flex-wrap gap-1.5">
              {suggestions.map((sg) => (
                <button
                  key={sg.command}
                  type="button"
                  title={`${sg.source}${sg.docker ? ' · docker' : ''}`}
                  onClick={() => act(() => api.createRun(projectId, { name: sg.name, command: sg.command, port: sg.port ?? null, docker: sg.docker }))}
                  className="inline-flex items-center gap-1.5 border border-line px-2 py-1 text-[13px] hover:bg-sunken"
                >
                  <Plus className="size-3 text-accent" />
                  <span>{sg.command}</span>
                  {sg.port ? <span className="text-muted">:{sg.port}</span> : null}
                  {sg.docker ? <span className="text-[10px] text-muted">docker</span> : null}
                </button>
              ))}
            </div>
          </div>
        ) : null}
        {runs && !runs.length && editing !== 'new' && !suggestions?.length ? <p className="text-sm text-muted">{t('run.emptyHint')}</p> : null}
        <ErrorLine error={error} />
      </div>
      {focused?.job ? (
        <RunLogs key={focused.job.id} job={focused.job} title={focused.name} />
      ) : (
        <div className="flex flex-1 items-center justify-center p-6 text-center text-sm text-muted">{t('run.logsHint')}</div>
      )}
    </div>
  );
}

function RunLogs({ job, title }: { job: JobInfo; title: string }) {
  const [status, setStatus] = useState({ running: job.status === 'running', code: job.exitCode });
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-1.5 p-2">
      <div className="flex items-center gap-2 text-xs">
        <span className="text-muted">{t('run.logs')}:</span>
        <span className="truncate">{title}</span>
        <span className="ml-auto">
          <JobStatusLine status={status} />
        </span>
      </div>
      <JobTerminal job={job} fill onStatus={setStatus} />
    </div>
  );
}

function RunForm({
  initial,
  onSave,
  onCancel,
}: {
  initial?: RunState;
  onSave: (v: { name: string; command: string; port: number | null; docker: boolean }) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? '');
  const [command, setCommand] = useState(initial?.command ?? '');
  const [port, setPort] = useState(initial?.port ? String(initial.port) : '');
  const [docker, setDocker] = useState(initial?.docker ?? false);
  return (
    <div className="space-y-2 border border-line p-2">
      <div className="grid gap-2 sm:grid-cols-[1fr_7rem]">
        <Field label={t('common.name')}>
          <input autoFocus className={inputCls} value={name} onChange={(e) => setName(e.target.value)} placeholder="dev" />
        </Field>
        <Field label={t('run.port')}>
          <input className={inputCls} value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ''))} placeholder={t('run.portAuto')} inputMode="numeric" />
        </Field>
      </div>
      <Field label={t('run.command')} hint={t('run.commandHint')}>
        <input className={inputCls} value={command} onChange={(e) => setCommand(e.target.value)} placeholder="uv run fastapi dev main.py" autoCapitalize="off" autoCorrect="off" spellCheck={false} />
      </Field>
      <div className="text-xs">
        <Checkbox checked={docker} onChange={setDocker}>
          <span className="text-fg">{t('run.docker')}</span> <span className="text-muted">{t('run.dockerHint')}</span>
        </Checkbox>
      </div>
      <div className="flex justify-end gap-2">
        <Button size="sm" variant="ghost" onClick={onCancel}>
          {t('common.cancel')}
        </Button>
        <Button size="sm" variant="primary" disabled={!name.trim() || !command.trim()} onClick={() => onSave({ name, command, port: port ? Number(port) : null, docker })}>
          {t('common.save')}
        </Button>
      </div>
    </div>
  );
}

// ---------- preview ----------

/**
 * Preview tab (like Webview in Replit). What to show is a choice: running
 * commands with a port and any ports entered manually (the app is running in
 * Shell, docker compose…). Address bar, reload, open in a tab.
 */
export function PreviewPanel({ projectId, runs, onStartRun }: { projectId: string; runs: RunState[] | null; onStartRun: (r: RunState) => void }) {
  const live = (runs ?? []).filter((r) => isRunning(r) && r.previewPort);
  const [manual, setManual] = useLocalStorageState<number[]>(`hub.preview.ports.${projectId}`, []);
  const [manualPreview, setManualPreview] = useState<Record<number, number>>({});
  const targets = [
    ...live.map((r) => ({ key: `run:${r.id}`, label: `${r.name} :${r.activePort}`, port: r.activePort!, previewPort: r.previewPort! })),
    ...manual.filter((p) => !live.some((r) => r.activePort === p)).map((p) => ({ key: `port:${p}`, label: `:${p}`, port: p, previewPort: manualPreview[p] ?? null })),
  ];
  const [pick, setPick] = useState<string | null>(null);
  const current = targets.find((x) => x.key === pick) ?? targets[0] ?? null;
  const [path, setPath] = useState('/');
  const [draft, setDraft] = useState('/');
  const [nonce, setNonce] = useState(0);
  const [portInput, setPortInput] = useState('');
  const [error, setError] = useState<string | null>(null);

  // manual ports: the proxy is started on demand
  useEffect(() => {
    for (const p of manual)
      if (!manualPreview[p]) api.previewPort(projectId, p).then((r) => setManualPreview((m) => ({ ...m, [p]: r.previewPort })), (e) => setError((e as Error).message));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [manual.join(','), projectId]);
  useEffect(() => {
    setPath('/');
    setDraft('/');
  }, [current?.key]);

  const addPort = () => {
    const p = Number(portInput);
    if (!Number.isInteger(p) || p < 1 || p > 65535) return setError(t('preview.badPort'));
    setError(null);
    if (!manual.includes(p)) setManual([...manual, p]);
    setPick(`port:${p}`);
    setPortInput('');
  };
  const portForm = (
    <form
      className="flex items-center gap-1"
      onSubmit={(e) => {
        e.preventDefault();
        addPort();
      }}
    >
      <input
        value={portInput}
        onChange={(e) => setPortInput(e.target.value.replace(/\D/g, ''))}
        placeholder={t('preview.portPlaceholder')}
        inputMode="numeric"
        className="w-28 border border-line bg-bg px-2 py-0.5 text-[13px] outline-none focus:border-accent"
      />
      <Button size="sm" type="submit" disabled={!portInput}>
        {t('preview.openPort')}
      </Button>
    </form>
  );

  const url = current?.previewPort ? previewUrl(current.previewPort, path) : null;
  if (!current) {
    const withPort = (runs ?? []).filter((r) => r.port || r.activePort);
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-4 p-6 text-center">
        <p className="text-sm text-muted">{t('preview.notRunning')}</p>
        {withPort.length ? (
          <div className="flex flex-wrap justify-center gap-1.5">
            {withPort.map((r) => (
              <Button key={r.id} size="sm" onClick={() => onStartRun(r)}>
                <Play className="size-3.5 text-ok" /> {r.name} <span className="text-muted">:{r.port ?? r.activePort}</span>
              </Button>
            ))}
          </div>
        ) : null}
        <div className="space-y-1">
          <p className="text-xs text-muted">{t('preview.portHint')}</p>
          {portForm}
        </div>
        <ErrorLine error={error} />
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-1 border-b border-line px-1.5 py-1">
        <Select size="sm" value={current.key} onChange={setPick} options={targets.map((x) => ({ value: x.key, label: x.label }))} className="max-w-48" />
        <form
          className="flex min-w-32 flex-1 items-center border border-line bg-bg px-2"
          onSubmit={(e) => {
            e.preventDefault();
            setPath(draft.startsWith('/') ? draft : `/${draft}`);
            setNonce((n) => n + 1);
          }}
        >
          <input value={draft} onChange={(e) => setDraft(e.target.value)} className="min-w-0 flex-1 bg-transparent px-1 py-0.5 text-[13px] outline-none" spellCheck={false} autoCapitalize="off" />
        </form>
        <IconButton label={t('preview.reload')} onClick={() => setNonce((n) => n + 1)} className="size-7">
          <RotateCw className="size-3.5" />
        </IconButton>
        {url ? (
          <a href={url} target="_blank" rel="noopener" title={t('preview.newTab')} className="inline-flex size-7 items-center justify-center text-muted hover:bg-sunken hover:text-fg">
            <ExternalLink className="size-3.5" />
          </a>
        ) : null}
        {current.key.startsWith('port:') ? (
          <IconButton label={t('preview.removePort')} onClick={() => setManual(manual.filter((p) => p !== current.port))} className="size-7">
            <Trash2 className="size-3.5" />
          </IconButton>
        ) : null}
        <div className="ml-auto">{portForm}</div>
      </div>
      <ErrorLine error={error} />
      {url ? <iframe key={`${url}#${nonce}`} src={url} title={t('preview.title')} className="min-h-0 w-full flex-1 bg-white" /> : <Spinner />}
    </div>
  );
}

/** Value in localStorage (per-viewer convenience: manual preview ports). */
function useLocalStorageState<T>(key: string, initial: T): [T, (v: T) => void] {
  const [v, setV] = useState<T>(() => {
    try {
      const raw = localStorage.getItem(key);
      return raw ? (JSON.parse(raw) as T) : initial;
    } catch {
      return initial;
    }
  });
  return [
    v,
    (next: T) => {
      setV(next);
      try {
        localStorage.setItem(key, JSON.stringify(next));
      } catch {}
    },
  ];
}

// ---------- shell ----------

/**
 * Shell tab: several shells as tabs. "+" opens another one (old ones
 * stay alive), × closes. After a page reload it reconnects to the
 * open ones. A shell without viewers is closed after 30 minutes.
 */
export function ShellPanel({ scope }: { scope: { projectId: string } | { groupId: string } }) {
  const [jobs, setJobs] = useState<JobInfo[] | null>(null);
  const [active, setActive] = useState<string | null>(null);
  const [status, setStatus] = useState<Record<string, { running: boolean; code: number | null }>>({});
  const [error, setError] = useState<string | null>(null);
  const key = 'projectId' in scope ? scope.projectId : scope.groupId;

  const open = useCallback(
    async (fresh: boolean) => {
      setError(null);
      try {
        const job = await api.shell(scope, { ...ptySize(), fresh });
        setJobs((list) => (list?.some((j) => j.id === job.id) ? list : [...(list ?? []), job]));
        setActive(job.id);
      } catch (e) {
        setError((e as Error).message);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [key],
  );
  useEffect(() => {
    api.shells(scope).then(
      (list) => {
        if (list.length) {
          setJobs(list);
          setActive(list[0]!.id);
        } else void open(false);
      },
      (e) => setError((e as Error).message),
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const close = (id: string) => {
    void api.jobKill(id).catch(() => {});
    const rest = (jobs ?? []).filter((j) => j.id !== id);
    setJobs(rest);
    if (active === id) setActive(rest[rest.length - 1]?.id ?? null);
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-stretch border-b border-line">
        <div className="flex min-w-0 flex-1 overflow-x-auto [scrollbar-width:none]">
          {(jobs ?? []).map((j) => {
            const st = status[j.id];
            return (
              <div key={j.id} className={`group flex shrink-0 items-center border-r border-line ${active === j.id ? 'bg-surface text-fg' : 'text-muted'}`}>
                <button type="button" onClick={() => setActive(j.id)} className="inline-flex items-center gap-1.5 py-1.5 pr-1 pl-2.5 text-[13px]">
                  <span className={`size-1.5 rounded-full ${st && !st.running ? 'bg-line' : 'bg-ok'}`} />
                  {j.title}
                </button>
                <button type="button" title={t('shell.close')} onClick={() => close(j.id)} className="inline-flex size-6 items-center justify-center hover:text-danger">
                  <X className="size-3" />
                </button>
              </div>
            );
          })}
        </div>
        <button type="button" title={t('shell.new')} onClick={() => void open(true)} className="inline-flex shrink-0 items-center gap-1 px-2.5 text-[13px] text-muted hover:bg-sunken hover:text-fg">
          <Plus className="size-3.5" /> <span className="hidden sm:inline">{t('shell.new')}</span>
        </button>
      </div>
      <ErrorLine error={error} />
      {jobs === null ? (
        <div className="p-3">
          <Spinner />
        </div>
      ) : jobs.length ? (
        jobs.map((j) => (
          <div key={j.id} className={`min-h-0 flex-1 flex-col p-2 ${active === j.id ? 'flex' : 'hidden'}`}>
            <JobTerminal job={j} fill onStatus={(st) => setStatus((m) => ({ ...m, [j.id]: st }))} />
          </div>
        ))
      ) : (
        <div className="flex flex-1 items-center justify-center">
          <Button size="sm" onClick={() => void open(true)}>
            <Plus className="size-3.5" /> {t('shell.new')}
          </Button>
        </div>
      )}
      <p className="shrink-0 px-2 pb-1 text-[11px] text-muted">{t('shell.hint')}</p>
    </div>
  );
}

/** One shell as a workbench tab: the terminal of that job, nothing else (the tab is its title). */
export function ShellTab({ jobId, title, onExit }: { jobId: string; title: string; onExit: () => void }) {
  // the stream's snapshot brings the real state; this is just enough to attach
  const job = useMemo<JobInfo>(() => ({ id: jobId, title, command: '', status: 'running', exitCode: null, startedAt: '' }), [jobId, title]);
  return (
    <div className="flex min-h-0 flex-1 flex-col p-2">
      <JobTerminal job={job} fill onExit={onExit} />
    </div>
  );
}

// ---------- environment ----------

/** Project environment (in settings): where Run/Shell execute, sandbox, secrets for agents. */
export function EnvironmentSection({ projectId }: { projectId: string }) {
  const [env, setEnv] = useState<EnvironmentStatus | null>(null);
  const [image, setImage] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setError(null);
    api.environment(projectId).then((e) => {
      setEnv(e);
      setImage(e.runtime.image);
    }, (e) => setError((e as Error).message));
  }, [projectId]);

  const save = async (patch: Parameters<typeof api.setEnvironment>[1]) => {
    setBusy(true);
    setError(null);
    try {
      const e = await api.setEnvironment(projectId, patch);
      setEnv(e);
      setImage(e.runtime.image);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const rt = env?.runtime;
  return (
    <>
      {!env || !rt ? (
        <div className="flex justify-center py-6">{error ? <ErrorLine error={error} /> : <Spinner />}</div>
      ) : (
        <div className="space-y-5">
          <div>
            <div className="mb-1 text-sm font-medium">{t('env.where')}</div>
            <p className="mb-2 text-xs text-muted">{t('env.whereHint')}</p>
            <div className="flex rounded-lg bg-sunken p-0.5 text-sm">
              {(['local', 'docker'] as const).map((k) => (
                <button
                  key={k}
                  type="button"
                  disabled={busy || (k === 'docker' && !env.dockerAvailable)}
                  onClick={() => k !== rt.environment && void save({ environment: k })}
                  className={`flex flex-1 items-center justify-center gap-1.5 rounded-md px-3 py-1.5 disabled:opacity-50 ${rt.environment === k ? 'bg-surface font-medium text-fg' : 'text-muted hover:text-fg'}`}
                >
                  {k === 'local' ? <TerminalIcon className="size-3.5" /> : <Box className="size-3.5" />}
                  {t(`env.${k}`)}
                </button>
              ))}
            </div>
            {!env.dockerAvailable ? <p className="mt-1.5 text-xs text-muted">{t('env.noDocker')}</p> : null}
          </div>

          {rt.environment === 'docker' ? (
            <div className="space-y-2 rounded-xl border border-line p-3">
              <Field label={t('env.image')} hint={env.devcontainerImage ? t('env.devcontainer', { image: env.devcontainerImage }) : t('env.imageHint')}>
                <div className="flex gap-2">
                  <input className={`${inputCls} font-mono text-sm`} value={image} onChange={(e) => setImage(e.target.value)} spellCheck={false} autoCapitalize="off" />
                  <Button size="sm" disabled={busy || image.trim() === rt.image} onClick={() => void save({ image })}>
                    {t('common.save')}
                  </Button>
                </div>
              </Field>
              {env.container ? (
                <div className="flex flex-wrap items-center gap-2 text-sm">
                  <span className={`size-2 rounded-full ${env.container.state === 'running' ? 'bg-ok' : 'bg-line'}`} />
                  <span className="font-mono text-xs">{env.container.name}</span>
                  <span className="text-muted">{t(`env.container.${env.container.state}`)}</span>
                  {env.container.state !== 'absent' ? (
                    <Button size="sm" variant="ghost" className="ml-auto" disabled={busy} onClick={() => api.removeContainer(projectId).then(setEnv, (e) => setError((e as Error).message))}>
                      <Trash2 className="size-3.5" /> {t('env.removeContainer')}
                    </Button>
                  ) : null}
                </div>
              ) : null}
              <p className="text-xs text-muted">{t('env.dockerHint')}</p>
            </div>
          ) : null}

          <Switch
            checked={rt.sandbox}
            disabled={busy || (!env.sandboxAvailable && !rt.sandbox)}
            onChange={(v) => void save({ sandbox: v })}
            label={t('env.sandbox')}
            hint={env.sandboxAvailable ? t('env.sandboxHint') : t('env.noSandbox')}
          />
          <Switch checked={rt.shareSecretsWithAgents} disabled={busy} onChange={(v) => void save({ shareSecretsWithAgents: v })} label={t('env.shareSecrets')} hint={t('env.shareSecretsHint')} />
          <p className="flex items-start gap-1.5 text-xs text-muted">
            <ShieldCheck className="mt-0.5 size-3.5 shrink-0" /> {t('env.restartNote')}
          </p>
          <ErrorLine error={error} />
        </div>
      )}
    </>
  );
}

// ---------- secrets ----------

/** Secrets tab: secret names, add/replace/delete (values are never shown). */
export function SecretsPanel({ projectId }: { projectId: string }) {
  const [data, setData] = useState<{ secrets: SecretInfo[]; unreadable: string[]; keySource: 'env' | 'file' } | null>(null);
  const [name, setName] = useState('');
  const [value, setValue] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(() => api.secrets(projectId).then(setData, (e) => setError((e as Error).message)), [projectId]);
  useEffect(() => {
    setError(null);
    void load();
  }, [load]);

  const add = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.setSecret(projectId, name.trim(), value);
      setName('');
      setValue('');
      await load();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-3">
      <div className="mx-auto max-w-2xl space-y-4">
        <p className="text-sm text-muted">{t('secrets.hint')}</p>
        {data?.secrets.length ? (
          <ul className="divide-y divide-line/60 rounded-xl border border-line">
            {data.secrets.map((s) => (
              <li key={s.name} className="flex items-center gap-2 px-3 py-2">
                <KeyRound className="size-4 shrink-0 text-accent" />
                <span className="min-w-0 flex-1 truncate font-mono text-sm">{s.name}</span>
                {data.unreadable.includes(s.name) ? <span className="text-xs text-danger">{t('secrets.unreadable')}</span> : <span className="font-mono text-xs text-muted">••••••</span>}
                <button type="button" className="text-xs text-muted hover:text-fg" onClick={() => setName(s.name)}>
                  {t('secrets.replace')}
                </button>
                <IconButton label={t('common.delete')} className="size-8" onClick={() => api.deleteSecret(projectId, s.name).then(() => void load(), (e) => setError((e as Error).message))}>
                  <Trash2 className="size-3.5" />
                </IconButton>
              </li>
            ))}
          </ul>
        ) : data ? (
          <p className="text-sm text-muted">{t('secrets.empty')}</p>
        ) : (
          <Spinner />
        )}
        <div className="space-y-2 rounded-xl border border-line p-3">
          <div className="grid gap-2 sm:grid-cols-[12rem_1fr]">
            <input
              className={`${inputCls} font-mono text-sm`}
              value={name}
              onChange={(e) => setName(e.target.value.replace(/[^A-Za-z0-9_]/g, '_'))}
              placeholder="API_KEY"
              autoCapitalize="off"
              spellCheck={false}
            />
            <input
              className={`${inputCls} font-mono text-sm`}
              type="password"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              placeholder={t('secrets.value')}
              autoComplete="new-password"
              onKeyDown={(e) => e.key === 'Enter' && name.trim() && value && void add()}
            />
          </div>
          <div className="flex justify-end">
            <Button size="sm" variant="primary" loading={busy} disabled={!name.trim() || !value} onClick={add}>
              {data?.secrets.some((s) => s.name === name.trim()) ? t('secrets.update') : t('secrets.add')}
            </Button>
          </div>
        </div>
        {data ? <p className="text-xs text-muted">{data.keySource === 'env' ? t('secrets.keyEnv') : t('secrets.keyFile')}</p> : null}
        <ErrorLine error={error} />
      </div>
    </div>
  );
}
