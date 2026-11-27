import { useCallback, useEffect, useState } from 'react';
import { AppWindow, Pencil, Play, Plus, Square, Star, Trash2 } from 'lucide-react';
import type { JobInfo, RunState, RunSuggestion } from '@hubness/shared';
import { api } from '../../lib/api';
import { t } from '../../i18n';
import { JobStatusLine, JobTerminal } from '../Terminal';
import { Button, ErrorLine, Field, inputCls, Spinner, Checkbox } from '../ui';
import { ptySize } from './shell';

/** Project run commands; polled every 3 s while something is running or the panel is open. */
/** Run commands of a project; `enabled: false` (no terminal access) never asks the server. */
export function useRuns(projectId: string, active: boolean, enabled = true) {
  const [runs, setRuns] = useState<RunState[] | null>(null);
  const reload = useCallback(() => (enabled ? api.runs(projectId).then(setRuns, () => {}) : Promise.resolve()), [projectId, enabled]);
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

export const isRunning = (r: RunState) => r.job?.status === 'running';

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
