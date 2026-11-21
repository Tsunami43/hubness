import { useCallback, useEffect, useState } from 'react';
import { KeyRound, Plus, Trash2 } from 'lucide-react';
import type { GitCredentialInfo } from '@hubness/shared';
import { api } from '../../lib/api';
import { t } from '../../i18n';
import { Button, ErrorLine, IconButton, Spinner } from '../../components/ui';
import { confirmDialog } from '../../components/Confirm';
import { GitLoginSheet } from '../../components/GitLogin';

/** Settings → Git access: logins per host for git over HTTPS (secrets are never shown). */
export function GitAccessPage() {
  const [list, setList] = useState<GitCredentialInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [edit, setEdit] = useState<GitCredentialInfo | 'new' | null>(null);

  const load = useCallback(() => api.gitCredentials().then(setList, (e) => setError((e as Error).message)), []);
  useEffect(() => {
    void load();
  }, [load]);

  const remove = async (c: GitCredentialInfo) => {
    if (!(await confirmDialog({ message: t('gitauth.deleteConfirm', { host: c.host }), confirmLabel: t('common.delete'), danger: true }))) return;
    api.deleteGitCredential(c.host).then(setList, (e) => setError((e as Error).message));
  };

  return (
    <div className="space-y-4">
      <p className="text-sm text-muted">{t('gitauth.intro')}</p>
      {list === null ? (
        <Spinner />
      ) : list.length ? (
        <ul className="divide-y divide-line overflow-hidden rounded-xl border border-line bg-surface">
          {list.map((c) => (
            <li key={c.host} className="flex items-center gap-3 px-4 py-2.5">
              <KeyRound className="size-4 shrink-0 text-accent" />
              <div className="min-w-0 flex-1">
                <div className="truncate font-mono text-sm">{c.host}</div>
                <div className="truncate text-xs text-muted">
                  {c.username} · {c.kind === 'token' ? t('gitauth.kindToken') : t('gitauth.kindPassword')}
                </div>
              </div>
              <Button size="sm" variant="ghost" onClick={() => setEdit(c)}>
                {t('gitauth.edit')}
              </Button>
              <IconButton label={t('common.delete')} className="size-8" onClick={() => void remove(c)}>
                <Trash2 className="size-3.5" />
              </IconButton>
            </li>
          ))}
        </ul>
      ) : (
        <p className="rounded-xl border border-line bg-surface px-4 py-3 text-sm text-muted">{t('gitauth.empty')}</p>
      )}
      <ErrorLine error={error} />
      <Button onClick={() => setEdit('new')}>
        <Plus className="size-4" />
        {t('gitauth.add')}
      </Button>
      <GitLoginSheet
        open={edit !== null}
        host={edit && edit !== 'new' ? edit.host : undefined}
        existing={edit && edit !== 'new' ? edit : undefined}
        onClose={() => setEdit(null)}
        onSaved={(l) => {
          setList(l);
          setEdit(null);
        }}
      />
    </div>
  );
}
