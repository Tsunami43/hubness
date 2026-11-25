import { useCallback, useEffect, useState } from 'react';
import { KeyRound, Trash2 } from 'lucide-react';
import type { SecretInfo } from '@hubness/shared';
import { api } from '../../lib/api';
import { t } from '../../i18n';
import { Button, ErrorLine, IconButton, inputCls, Spinner } from '../ui';

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
