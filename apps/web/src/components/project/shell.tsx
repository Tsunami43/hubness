import { useCallback, useEffect, useMemo, useState } from 'react';
import { Plus, X } from 'lucide-react';
import type { JobInfo } from '@hubness/shared';
import { api } from '../../lib/api';
import { t } from '../../i18n';
import { JobTerminal } from '../Terminal';
import { Button, ErrorLine, Spinner } from '../ui';

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
