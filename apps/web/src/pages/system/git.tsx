import { useCallback, useEffect, useRef, useState } from 'react';
import { CircleAlert, CircleCheck, KeyRound, Plus, RefreshCw, Trash2 } from 'lucide-react';
import type { GitCredentialInfo } from '@hubness/shared';
import { api } from '../../lib/api';
import { t } from '../../i18n';
import { Button, ErrorLine, IconButton, Spinner } from '../../components/ui';
import { confirmDialog } from '../../components/Confirm';
import { GitLoginSheet } from '../../components/GitLogin';
import { Header } from './parts';

/** The account's picture (kept by the hub, see git-login-check), or a key when there is none. */
function Avatar({ c }: { c: GitCredentialInfo }) {
  const [broken, setBroken] = useState(false);
  // a new check may bring a new picture: its time is part of the address
  const src = `/api/git-credentials/${encodeURIComponent(c.host)}/avatar?v=${encodeURIComponent(c.checkedAt ?? '')}`;
  useEffect(() => setBroken(false), [src]);
  return (
    <span className="flex size-9 shrink-0 items-center justify-center overflow-hidden rounded-full border border-line bg-sunken">
      {c.avatar && !broken ? <img src={src} alt="" className="size-full object-cover" onError={() => setBroken(true)} /> : <KeyRound className="size-4 text-accent" />}
    </span>
  );
}

/** What the last check said, in one line. */
function Status({ c, checking }: { c: GitCredentialInfo; checking: boolean }) {
  if (checking)
    return (
      <span className="inline-flex items-center gap-1 text-muted">
        <Spinner className="size-3" /> {t('gitauth.checking')}
      </span>
    );
  if (c.status === 'ok')
    return (
      <span className="inline-flex items-center gap-1 text-ok">
        <CircleCheck className="size-3.5 shrink-0" />
        {c.account && c.account !== c.username ? t('gitauth.signedInAs', { name: c.account }) : t('gitauth.signedIn')}
      </span>
    );
  if (c.status === 'failed')
    return (
      <span className="inline-flex items-start gap-1 text-danger">
        <CircleAlert className="mt-0.5 size-3.5 shrink-0" />
        {c.error || t('gitauth.refused')}
      </span>
    );
  return <span className="text-muted">{c.error || t('gitauth.notChecked')}</span>;
}

/** System → Git access: logins per host for git over HTTPS (secrets are never shown). */
export function GitSection() {
  const [list, setList] = useState<GitCredentialInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [edit, setEdit] = useState<GitCredentialInfo | 'new' | null>(null);
  const [checking, setChecking] = useState<Set<string>>(new Set());
  const listRef = useRef(list);
  listRef.current = list;

  /** Ask the host: is the login still accepted, who is it, what does it look like now. */
  const check = useCallback(async (host: string) => {
    setChecking((s) => new Set(s).add(host));
    try {
      setList(await api.checkGitCredential(host));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setChecking((s) => {
        const next = new Set(s);
        next.delete(host);
        return next;
      });
    }
  }, []);

  useEffect(() => {
    api.gitCredentials().then(
      (l) => {
        setList(l);
        // logins saved before the hub could check them (or by the GitHub sign-in): once, quietly
        for (const c of l) if (c.status === 'unknown' && !c.checkedAt) void check(c.host);
      },
      (e) => setError((e as Error).message),
    );
  }, [check]);

  const remove = async (c: GitCredentialInfo) => {
    if (!(await confirmDialog({ message: t('gitauth.deleteConfirm', { host: c.host }), confirmLabel: t('common.delete'), danger: true }))) return;
    api.deleteGitCredential(c.host).then(setList, (e) => setError((e as Error).message));
  };

  /** A saved login is checked at once: its state and picture show without another click. */
  const saved = (next: GitCredentialInfo[]) => {
    const before = new Map((listRef.current ?? []).map((c) => [c.host, c.updatedAt]));
    setList(next);
    setEdit(null);
    for (const c of next) if (before.get(c.host) !== c.updatedAt) void check(c.host);
  };

  return (
    <div className="space-y-4">
      <Header
        title={t('gitauth.title')}
        hint={t('gitauth.intro')}
        actions={
          <Button variant="primary" onClick={() => setEdit('new')}>
            <Plus className="size-4" />
            {t('gitauth.add')}
          </Button>
        }
      />
      {list === null ? (
        <Spinner />
      ) : list.length ? (
        <ul className="divide-y divide-line overflow-hidden rounded-xl border border-line bg-surface">
          {list.map((c) => (
            <li key={c.host} className="flex items-center gap-3 px-4 py-2.5">
              <Avatar c={c} />
              <div className="min-w-0 flex-1">
                <div className="truncate font-mono text-sm">{c.host}</div>
                <div className="truncate text-xs text-muted">
                  {c.username} · {c.kind === 'token' ? t('gitauth.kindToken') : t('gitauth.kindPassword')}
                </div>
                <div className="mt-0.5 text-xs">
                  <Status c={c} checking={checking.has(c.host)} />
                </div>
              </div>
              <IconButton label={t('gitauth.check')} className="size-8" disabled={checking.has(c.host)} onClick={() => void check(c.host)}>
                <RefreshCw className={`size-3.5 ${checking.has(c.host) ? 'animate-spin' : ''}`} />
              </IconButton>
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
      <GitLoginSheet
        open={edit !== null}
        host={edit && edit !== 'new' ? edit.host : undefined}
        existing={edit && edit !== 'new' ? edit : undefined}
        onClose={() => setEdit(null)}
        onSaved={saved}
      />
    </div>
  );
}
