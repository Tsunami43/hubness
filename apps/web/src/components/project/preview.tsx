import { useEffect, useState } from 'react';
import { Play, Plus, RotateCw, SquareTerminal, Trash2 } from 'lucide-react';
import { BrowserFrame } from './BrowserFrame';
import type { RunState } from '@hubness/shared';
import { api } from '../../lib/api';
import { t } from '../../i18n';
import { Button, ErrorLine, IconButton, Spinner } from '../ui';
import { Select } from '../Select';
import { useLocalStorage } from '../../lib/hooks';
import { isRunning } from './runs';
import { previewIdle, startable } from '../../lib/previewState';

export const previewUrl = (port: number, path = '/') => `${window.location.protocol}//${window.location.hostname}:${port}${path}`;

/**
 * Preview tab (like Webview in Replit). What to show is a choice: running
 * commands with a port and any ports entered manually (the app is running in
 * Shell, docker compose…); the page itself is in a BrowserFrame.
 */
export function PreviewPanel({
  projectId,
  runs,
  onStartRun,
  onShowRun,
}: {
  projectId: string;
  runs: RunState[] | null;
  /** Start a command and stay here: the page shows up when its port is known. */
  onStartRun: (r: RunState) => void;
  /** Go to the Run tab: the output of a command, or (null) the list to add one. */
  onShowRun: (r: RunState | null) => void;
}) {
  const live = (runs ?? []).filter((r) => isRunning(r) && r.previewPort);
  const [manual, setManual] = useLocalStorage<number[]>(`hub.preview.ports.${projectId}`, []);
  // proxies of the manual ports, per project: a port of another project is another app
  const [proxies, setProxies] = useState<{ projectId: string; ports: Record<number, number> }>({ projectId, ports: {} });
  const manualPreview = proxies.projectId === projectId ? proxies.ports : {};
  const [retry, setRetry] = useState(0);
  const targets = [
    ...live.map((r) => ({ key: `run:${r.id}`, label: `${r.name} :${r.activePort}`, port: r.activePort!, previewPort: r.previewPort! })),
    ...manual.filter((p) => !live.some((r) => r.activePort === p)).map((p) => ({ key: `port:${p}`, label: `:${p}`, port: p, previewPort: manualPreview[p] ?? null })),
  ];
  const [pick, setPick] = useState<string | null>(null);
  const current = targets.find((x) => x.key === pick) ?? targets[0] ?? null;
  const [adding, setAdding] = useState(false);
  const [portInput, setPortInput] = useState('');
  const [error, setError] = useState<string | null>(null);

  // manual ports: the proxy is started on demand
  useEffect(() => {
    let stale = false;
    setError(null);
    for (const p of manual)
      if (!manualPreview[p])
        api.previewPort(projectId, p).then(
          (r) => !stale && setProxies((cur) => ({ projectId, ports: { ...(cur.projectId === projectId ? cur.ports : {}), [p]: r.previewPort } })),
          (e) => !stale && setError((e as Error).message),
        );
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [manual.join(','), projectId, retry]);

  const addPort = () => {
    const p = Number(portInput);
    if (!Number.isInteger(p) || p < 1 || p > 65535) return setError(t('preview.badPort'));
    setError(null);
    if (!manual.includes(p)) setManual([...manual, p]);
    setPick(`port:${p}`);
    setPortInput('');
    setAdding(false);
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

  if (!current) {
    const all = runs ?? [];
    const idle = previewIdle(all);
    const offer = startable(all);
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-4 overflow-auto p-6 text-center">
        {idle.kind === 'waiting' ? (
          <>
            <Spinner />
            <p className="text-sm">{t('preview.waiting', { name: idle.run.name })}</p>
            <p className="max-w-md text-xs text-muted">{t('preview.waitingHint')}</p>
            <Button size="sm" onClick={() => onShowRun(idle.run)}>
              <SquareTerminal className="size-3.5" /> {t('preview.showLogs')}
            </Button>
          </>
        ) : (
          <>
            {idle.kind === 'failed' ? (
              <div className="space-y-2">
                <p className="text-sm text-danger">{t('preview.failed', { name: idle.run.name, code: idle.code })}</p>
                <Button size="sm" onClick={() => onShowRun(idle.run)}>
                  <SquareTerminal className="size-3.5" /> {t('preview.showLogs')}
                </Button>
              </div>
            ) : (
              <p className="text-sm text-muted">{t('preview.notRunning')}</p>
            )}
            <p className="max-w-md text-xs text-muted">{t('preview.how')}</p>
            {offer.length ? (
              <div className="flex flex-wrap justify-center gap-1.5">
                {offer.map((r) => (
                  <Button key={r.id} size="sm" onClick={() => onStartRun(r)}>
                    <Play className="size-3.5 text-ok" /> {r.name} {r.port || r.activePort ? <span className="text-muted">:{r.port ?? r.activePort}</span> : null}
                  </Button>
                ))}
              </div>
            ) : runs && !runs.length ? (
              <Button size="sm" onClick={() => onShowRun(null)}>
                <Plus className="size-3.5" /> {t('run.add')}
              </Button>
            ) : null}
          </>
        )}
        <div className="space-y-1">
          <p className="text-xs text-muted">{t('preview.portHint')}</p>
          <div className="flex justify-center">{portForm}</div>
        </div>
        <ErrorLine error={error} />
      </div>
    );
  }
  const toolbar = (
    <>
      <Select
        size="sm"
        value={adding ? '__add' : current.key}
        onChange={(v) => (v === '__add' ? setAdding(true) : (setAdding(false), setPick(v)))}
        options={[...targets.map((x) => ({ value: x.key, label: x.label })), { value: '__add', label: t('preview.otherPort') }]}
        className="max-w-48"
      />
      {adding ? portForm : null}
      {current.key.startsWith('port:') && !adding ? (
        <IconButton label={t('preview.removePort')} onClick={() => setManual(manual.filter((p) => p !== current.port))} className="size-7">
          <Trash2 className="size-3.5" />
        </IconButton>
      ) : null}
    </>
  );
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {current.previewPort ? (
        <BrowserFrame key={current.previewPort} projectId={projectId} previewPort={current.previewPort} appPort={current.port} toolbar={toolbar} />
      ) : (
        // no proxy (yet, or it could not be started): the target can still be changed or removed
        <>
          <div className="flex shrink-0 flex-wrap items-center gap-1 border-b border-line px-1.5 py-1">{toolbar}</div>
          <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6">
            {error ? (
              <Button size="sm" onClick={() => setRetry((n) => n + 1)}>
                <RotateCw className="size-3.5" />
                {t('common.retry')}
              </Button>
            ) : (
              <Spinner />
            )}
          </div>
        </>
      )}
      <ErrorLine error={error} />
    </div>
  );
}
