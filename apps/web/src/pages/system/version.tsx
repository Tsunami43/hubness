import { useEffect, useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { api } from '../../lib/api';
import { useApi } from '../../lib/query';
import { updateStep } from '../../lib/hubUpdate';
import { Button, IconButton } from '../../components/ui';
import { t } from '../../i18n';

type Phase = { kind: 'idle' } | { kind: 'updating'; from: string } | { kind: 'installed' } | { kind: 'error'; message: string };

/** The hub's version above the host card, with an "Update" button when a release is newer. */
export function HubVersionCard() {
  const { data: v, setData, error } = useApi(['system', 'version'], () => api.hubVersion(), { staleTime: 60_000 });
  const [checking, setChecking] = useState(false);
  const [local, setPhase] = useState<Phase>({ kind: 'idle' });
  // the server's state counts until this page starts an update (e.g. after a reload)
  const phase: Phase =
    local.kind !== 'idle' || !v
      ? local
      : v.update.state === 'running'
        ? { kind: 'updating', from: v.version }
        : v.update.state === 'installed'
          ? { kind: 'installed' }
          : v.update.state === 'failed'
            ? { kind: 'error', message: v.update.message ?? t('ver.failed') }
            : local;
  const polling = phase.kind === 'updating' ? phase.from : null;

  // after "Update": poll until the restarted hub answers with the new version
  useEffect(() => {
    if (polling === null) return;
    const at = Date.now();
    const id = setInterval(async () => {
      const now = await api.hubVersion().catch(() => null);
      const step = updateStep(polling, now, Date.now() - at);
      if (step === 'reload') window.location.reload();
      else if (step === 'failed') setPhase({ kind: 'error', message: now?.update.message ?? t('ver.failed') });
      else if (step === 'installed') setPhase({ kind: 'installed' });
      else if (step === 'timeout') setPhase({ kind: 'error', message: t('ver.timeout') });
    }, 2000);
    return () => clearInterval(id);
  }, [polling]);

  if (!v) return error ? null : <div className="mb-3 h-[60px] border border-line bg-surface" />;

  const check = async () => {
    setChecking(true);
    try {
      setData(await api.hubVersion(true));
    } catch {
      // the card keeps the last answer
    } finally {
      setChecking(false);
    }
  };
  const update = async () => {
    try {
      await api.hubUpdate();
      setPhase({ kind: 'updating', from: v.version });
    } catch (e) {
      setPhase({ kind: 'error', message: (e as Error).message });
    }
  };

  const status =
    phase.kind === 'updating'
      ? t('ver.updating', { version: v.latest ?? '' })
      : phase.kind === 'installed'
        ? t('ver.installed')
        : v.updateAvailable
          ? t('ver.available', { version: v.latest ?? '' })
          : v.latest
            ? t('ver.upToDate')
            : t('ver.unknown');

  return (
    <section className="mb-3 border border-line bg-surface px-4 py-3">
      <div className="flex items-center gap-3">
        <img src="/brand/hubness-icon.svg" alt="" className="size-8 shrink-0 select-none" draggable={false} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold">
            hubness <span className="font-normal text-muted">{v.version}</span>
          </div>
          <div className={`truncate text-xs ${v.updateAvailable || phase.kind === 'installed' ? 'text-warn' : 'text-muted'}`}>
            {status}
            {v.updateAvailable && v.releaseUrl && phase.kind === 'idle' ? (
              <>
                {' · '}
                <a href={v.releaseUrl} target="_blank" rel="noreferrer" className="underline hover:text-fg">
                  {t('ver.changes')}
                </a>
              </>
            ) : null}
          </div>
        </div>
        {v.updateAvailable && v.canUpdate && phase.kind !== 'installed' ? (
          <Button variant="primary" size="sm" loading={phase.kind === 'updating'} onClick={() => void update()}>
            {t('ver.update')}
          </Button>
        ) : phase.kind === 'idle' ? (
          <IconButton label={t('ver.check')} onClick={() => void check()} disabled={checking}>
            <RefreshCw className={`size-4 ${checking ? 'animate-spin' : ''}`} />
          </IconButton>
        ) : null}
      </div>
      {v.updateAvailable && !v.canUpdate ? <p className="mt-2 text-xs text-muted">{t('ver.manual')}</p> : null}
      {phase.kind === 'error' ? <p className="mt-2 font-mono text-xs break-words whitespace-pre-wrap text-danger">{phase.message}</p> : null}
    </section>
  );
}
