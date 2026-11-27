import { useEffect, useState } from 'react';
import { Play, Trash2 } from 'lucide-react';
import { BrowserFrame } from './BrowserFrame';
import type { RunState } from '@hubness/shared';
import { api } from '../../lib/api';
import { t } from '../../i18n';
import { Button, ErrorLine, IconButton, Spinner } from '../ui';
import { Select } from '../Select';
import { useLocalStorage } from '../../lib/hooks';
import { isRunning } from './runs';

export const previewUrl = (port: number, path = '/') => `${window.location.protocol}//${window.location.hostname}:${port}${path}`;

/**
 * Preview tab (like Webview in Replit). What to show is a choice: running
 * commands with a port and any ports entered manually (the app is running in
 * Shell, docker compose…); the page itself is in a BrowserFrame.
 */
export function PreviewPanel({ projectId, runs, onStartRun }: { projectId: string; runs: RunState[] | null; onStartRun: (r: RunState) => void }) {
  const live = (runs ?? []).filter((r) => isRunning(r) && r.previewPort);
  const [manual, setManual] = useLocalStorage<number[]>(`hub.preview.ports.${projectId}`, []);
  const [manualPreview, setManualPreview] = useState<Record<number, number>>({});
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
    for (const p of manual)
      if (!manualPreview[p]) api.previewPort(projectId, p).then((r) => setManualPreview((m) => ({ ...m, [p]: r.previewPort })), (e) => setError((e as Error).message));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [manual.join(','), projectId]);

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
      {current.previewPort ? (
        <BrowserFrame
          projectId={projectId}
          previewPort={current.previewPort}
          appPort={current.port}
          toolbar={
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
          }
        />
      ) : (
        <Spinner />
      )}
      <ErrorLine error={error} />
    </div>
  );
}
