import { useEffect, useState } from 'react';
import { ExternalLink, Play, RotateCw, Trash2 } from 'lucide-react';
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
 * Shell, docker compose…). Address bar, reload, open in a tab.
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
