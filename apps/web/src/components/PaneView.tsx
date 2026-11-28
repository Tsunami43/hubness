import { useEffect, useState } from 'react';
import { ExternalLink, FileText, GitCompare, Globe, RotateCw, X } from 'lucide-react';
import type { FileContent, Pane } from '@hubness/shared';
import { api } from '../lib/api';
import { t } from '../i18n';
import { DiffView } from './DiffView';
import { CodeLine, useTokens } from './Code';
import { ErrorLine, IconButton, Spinner } from './ui';
import { localApp, reachable } from '../lib/previewState';
import { Unreachable } from './project/BrowserFrame';

/**
 * Where a page the agent shows is loaded from. An app on the hub's machine
 * (`http://localhost:5173`) is not on the user's machine: it goes through the project's
 * preview, like the Preview tab. `unreachable` — the browser can't open that preview port.
 */
type Shown = { state: 'loading' } | { state: 'ready'; src: string } | { state: 'unreachable'; previewPort: number };

function useShownUrl(pane: Pane, projectId: string | null, reloadKey: number): Shown {
  const app = pane.kind === 'url' && projectId ? localApp(pane.target) : null;
  const [shown, setShown] = useState<Shown>({ state: 'loading' });
  useEffect(() => {
    if (pane.kind !== 'url') return;
    if (!app || !projectId) return setShown({ state: 'ready', src: pane.target });
    let stale = false;
    setShown({ state: 'loading' });
    api
      .previewPort(projectId, app.port)
      .then(async (r) => {
        const origin = `${window.location.protocol}//${window.location.hostname}:${r.previewPort}`;
        const ok = await reachable(origin);
        if (!stale) setShown(ok ? { state: 'ready', src: origin + app.path } : { state: 'unreachable', previewPort: r.previewPort });
      })
      // no preview to be had (no free port, a project that is gone): the address as it is
      .catch(() => {
        if (!stale) setShown({ state: 'ready', src: pane.target });
      });
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pane.kind, pane.target, projectId, reloadKey]);
  return shown;
}

/** Panel next to the chat: the agent shows a site preview, a file or a diff (MCP open_pane). */
export function PaneView({ sessionId, projectId, pane, onClose }: { sessionId: string; projectId: string | null; pane: Pane; onClose: () => void }) {
  const [file, setFile] = useState<FileContent | null>(null);
  const [diff, setDiff] = useState<string | null>(null);
  const tokens = useTokens(file && !file.binary ? file.content.replace(/\n$/, '') : '', pane.kind === 'file' ? { path: pane.target } : null);
  const [error, setError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const shown = useShownUrl(pane, projectId, reloadKey);

  useEffect(() => {
    setFile(null);
    setDiff(null);
    setError(null);
    if (pane.kind === 'file') api.paneFile(sessionId, pane.target).then(setFile, (e) => setError((e as Error).message));
    if (pane.kind === 'diff') api.paneDiff(sessionId, pane.target).then((r) => setDiff(r.diff), (e) => setError((e as Error).message));
  }, [sessionId, pane, reloadKey]);

  const Icon = pane.kind === 'url' ? Globe : pane.kind === 'diff' ? GitCompare : FileText;

  return (
    <div className="flex min-h-0 w-full flex-1 flex-col bg-surface">
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-line/60 px-3">
        <Icon className="size-4 shrink-0 text-accent" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium">{pane.title || pane.target}</div>
          {pane.title ? <div className="truncate font-mono text-[11px] text-muted">{pane.target}</div> : null}
        </div>
        <IconButton label={t('pane.reload')} className="size-8" onClick={() => setReloadKey((k) => k + 1)}>
          <RotateCw className="size-4" />
        </IconButton>
        {pane.kind === 'url' ? (
          <a href={shown.state === 'ready' ? shown.src : pane.target} target="_blank" rel="noreferrer" aria-label={t('pane.openTab')} title={t('pane.openTab')} className="inline-flex size-8 items-center justify-center rounded-lg hover:bg-sunken">
            <ExternalLink className="size-4" />
          </a>
        ) : null}
        <IconButton label={t('common.close')} className="size-8" onClick={onClose}>
          <X className="size-4" />
        </IconButton>
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        {error ? (
          <div className="p-3">
            <ErrorLine error={error} />
          </div>
        ) : pane.kind === 'url' && shown.state === 'loading' ? (
          <div className="p-4"><Spinner /></div>
        ) : pane.kind === 'url' && shown.state === 'unreachable' ? (
          <Unreachable previewPort={shown.previewPort} onRetry={() => setReloadKey((k) => k + 1)} />
        ) : pane.kind === 'url' && shown.state === 'ready' ? (
          <iframe
            key={reloadKey}
            src={shown.src}
            title={pane.title || pane.target}
            className="h-full w-full bg-white"
            sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-modals"
          />
        ) : pane.kind === 'diff' ? (
          diff === null ? <div className="p-4"><Spinner /></div> : <DiffView diff={diff} />
        ) : file === null ? (
          <div className="p-4"><Spinner /></div>
        ) : file.binary ? (
          <div className="p-4 text-sm text-muted">{t('files.binary')}</div>
        ) : (
          <table className="font-mono text-[12px] leading-[1.6]">
            <tbody>
              {file.content.replace(/\n$/, '').split('\n').map((l, i) => (
                <tr key={i}>
                  <td className="sticky left-0 w-px bg-surface px-2 text-right text-muted/60 select-none">{i + 1}</td>
                  <td className="pr-4 pl-2 whitespace-pre">
                    <CodeLine tokens={tokens?.[i]} text={l} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
