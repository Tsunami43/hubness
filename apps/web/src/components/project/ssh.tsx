import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { KeyRound, Plus, X } from 'lucide-react';
import type { SshBinding, SshConnection } from '@hubness/shared';
import { api } from '../../lib/api';
import { t } from '../../i18n';
import { Button, ErrorLine, IconButton, inputCls, Spinner } from '../ui';
import { EnvBadge } from '../../pages/system/ssh';

type Scope = { projectId: string } | { groupId: string };

/**
 * SSH tab of a project or group: which of the hub's connections its agent sessions get, and
 * what each is for here (the note goes to the agent next to the connection's description).
 */
export function SshBindingsPanel({ scope }: { scope: Scope }) {
  const [all, setAll] = useState<SshConnection[] | null>(null);
  const [list, setList] = useState<{ connectionId: string; note: string }[] | null>(null);
  const [saved, setSaved] = useState<string>('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const key = 'projectId' in scope ? scope.projectId : scope.groupId;

  const apply = useCallback((b: SshBinding[]) => {
    const next = b.map((x) => ({ connectionId: x.connection.id, note: x.note }));
    setList(next);
    setSaved(JSON.stringify(next));
  }, []);
  useEffect(() => {
    setError(null);
    Promise.all([api.sshConnections(), api.sshBindings(scope)]).then(([c, b]) => {
      setAll(c);
      apply(b);
    }, (e) => setError((e as Error).message));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const save = async () => {
    if (!list) return;
    setBusy(true);
    setError(null);
    try {
      apply(await api.setSshBindings(scope, list));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  if (!all || !list)
    return (
      <div className="p-3">
        {error ? <ErrorLine error={error} /> : <Spinner />}
      </div>
    );
  const byId = new Map(all.map((c) => [c.id, c]));
  const free = all.filter((c) => !list.some((b) => b.connectionId === c.id));
  const dirty = JSON.stringify(list) !== saved;

  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-3">
      <div className="mx-auto max-w-2xl space-y-4">
        <p className="text-sm text-muted">{t('ssh.bindHint')}</p>
        {list.length ? (
          <ul className="space-y-2">
            {list.map((b, i) => {
              const c = byId.get(b.connectionId);
              if (!c) return null;
              return (
                <li key={b.connectionId} className="space-y-2 rounded-xl border border-line p-3">
                  <div className="flex flex-wrap items-center gap-2">
                    <KeyRound className="size-4 shrink-0 text-accent" />
                    <span className="font-mono font-medium">{c.name}</span>
                    <EnvBadge env={c.environment} />
                    <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted">
                      {c.username}@{c.host}
                    </span>
                    <IconButton label={t('ssh.unbind')} className="size-8" onClick={() => setList(list.filter((_, j) => j !== i))}>
                      <X className="size-3.5" />
                    </IconButton>
                  </div>
                  {c.description ? <p className="text-xs text-muted">{c.description}</p> : null}
                  <input
                    className={`${inputCls} text-sm`}
                    value={b.note}
                    onChange={(e) => setList(list.map((x, j) => (j === i ? { ...x, note: e.target.value } : x)))}
                    placeholder={t('ssh.notePlaceholder')}
                  />
                </li>
              );
            })}
          </ul>
        ) : (
          <p className="text-sm text-muted">{t('ssh.noBindings')}</p>
        )}
        {free.length ? (
          <div className="flex flex-wrap gap-2">
            {free.map((c) => (
              <Button key={c.id} size="sm" onClick={() => setList([...list, { connectionId: c.id, note: '' }])}>
                <Plus className="size-3.5" />
                <span className="font-mono">{c.name}</span>
                <EnvBadge env={c.environment} />
              </Button>
            ))}
          </div>
        ) : null}
        <p className="text-xs text-muted">
          {t('ssh.manageHint')}{' '}
          <Link to="/system/ssh" className="text-accent hover:underline">
            {t('nav.system')} → SSH
          </Link>
        </p>
        <div className="flex justify-end">
          <Button variant="primary" loading={busy} disabled={!dirty} onClick={() => void save()}>
            {t('common.save')}
          </Button>
        </div>
        <ErrorLine error={error} />
      </div>
    </div>
  );
}
