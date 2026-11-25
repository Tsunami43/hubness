import { useEffect, useState } from 'react';
import { Box, ShieldCheck, Trash2, Terminal as TerminalIcon } from 'lucide-react';
import type { EnvironmentStatus } from '@hubness/shared';
import { api } from '../../lib/api';
import { t } from '../../i18n';
import { Button, ErrorLine, Field, inputCls, Spinner, Switch } from '../ui';

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
