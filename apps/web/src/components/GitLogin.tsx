import { useEffect, useState } from 'react';
import { ExternalLink } from 'lucide-react';
import type { GitAuthProblem, GitCredentialInfo, GitCredentialKind } from '@hubness/shared';
import { api } from '../lib/api';
import { t } from '../i18n';
import { Button, ErrorLine, Field, inputCls, Sheet } from './ui';

/** Where to create a personal access token on known hostings. */
function tokenPage(host: string): string | null {
  if (host === 'github.com' || host.startsWith('github.')) return `https://${host}/settings/tokens`;
  if (host === 'gitlab.com' || host.startsWith('gitlab.')) return `https://${host}/-/user_settings/personal_access_tokens`;
  return null;
}

/**
 * Sign in to a git host over HTTPS: token (recommended) or login + password. The
 * secret goes to the server once and is never shown again.
 */
export function GitLoginSheet({
  open,
  host: fixedHost,
  existing,
  saveLabel,
  onClose,
  onSaved,
}: {
  open: boolean;
  /** Fixed host (from an access error or an existing login); empty — the user types it. */
  host?: string;
  existing?: GitCredentialInfo;
  saveLabel?: string;
  onClose: () => void;
  onSaved: (list: GitCredentialInfo[]) => void;
}) {
  const [host, setHost] = useState('');
  const [kind, setKind] = useState<GitCredentialKind>('token');
  const [username, setUsername] = useState('');
  const [secret, setSecret] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setHost(fixedHost ?? '');
    setKind(existing?.kind ?? 'token');
    setUsername(existing?.username ?? '');
    setSecret('');
    setError(null);
    // opened from an access error: prefill the login already stored for the host
    if (existing || !fixedHost) return;
    let live = true;
    api.gitCredentials().then(
      (list) => {
        const c = list.find((x) => x.host === fixedHost);
        if (!live || !c) return;
        setKind(c.kind);
        setUsername((u) => u || c.username);
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [open, fixedHost, existing]);

  const h = (fixedHost ?? host).trim().toLowerCase();
  const page = h ? tokenPage(h) : null;
  const ready = h && username.trim() && secret;

  const save = async () => {
    if (!ready) return;
    setBusy(true);
    setError(null);
    try {
      onSaved(await api.setGitCredential(h, { username: username.trim(), kind, secret }));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet
      open={open}
      onClose={onClose}
      title={fixedHost ? t('gitauth.signIn', { host: fixedHost }) : t('gitauth.add')}
      footer={
        <>
          <span className="flex-1" />
          <Button variant="ghost" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" loading={busy} disabled={!ready} onClick={() => void save()}>
            {saveLabel ?? t('gitauth.save')}
          </Button>
        </>
      }
    >
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        {fixedHost ? null : (
          <Field label={t('gitauth.host')} hint={t('gitauth.hostHint')}>
            <input
              className={`${inputCls} font-mono`}
              value={host}
              onChange={(e) => setHost(e.target.value)}
              placeholder="gitlab.example.com"
              autoCapitalize="off"
              spellCheck={false}
              autoFocus
            />
          </Field>
        )}
        <div className="flex border border-line" role="radiogroup" aria-label={t('gitauth.method')}>
          {(['token', 'password'] as const).map((k) => (
            <button
              key={k}
              type="button"
              role="radio"
              aria-checked={kind === k}
              onClick={() => setKind(k)}
              className={`h-8 flex-1 px-3 text-sm ${kind === k ? 'bg-sunken text-accent' : 'text-muted hover:text-fg'}`}
            >
              {k === 'token' ? t('gitauth.token') : t('gitauth.password')}
            </button>
          ))}
        </div>
        <p className={`text-xs ${kind === 'password' ? 'text-warn' : 'text-muted'}`}>{kind === 'token' ? t('gitauth.tokenHint') : t('gitauth.passwordHint')}</p>
        {kind === 'token' && page ? (
          <a href={page} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 text-sm text-accent hover:underline">
            {t('gitauth.createToken', { host: h })}
            <ExternalLink className="size-3.5" />
          </a>
        ) : null}
        <Field label={t('gitauth.username')}>
          <input
            className={`${inputCls} font-mono`}
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoCapitalize="off"
            autoComplete="username"
            spellCheck={false}
            autoFocus={Boolean(fixedHost)}
          />
        </Field>
        <Field label={kind === 'token' ? t('gitauth.secretToken') : t('gitauth.secretPassword')} hint={existing ? t('gitauth.secretAgain') : undefined}>
          <input className={`${inputCls} font-mono`} type="password" value={secret} onChange={(e) => setSecret(e.target.value)} autoComplete="new-password" />
        </Field>
        <ErrorLine error={error} />
        <button type="submit" hidden />
      </form>
    </Sheet>
  );
}

/** Explains an access refusal from a git network operation, with a sign-in button for HTTPS. */
export function GitAuthNotice({ problem, onSignIn }: { problem: GitAuthProblem; onSignIn: () => void }) {
  const host = problem.host || '?';
  const text =
    problem.protocol === 'https'
      ? problem.reason === 'rejected'
        ? t('gitauth.rejected', { host })
        : t('gitauth.missing', { host })
      : problem.reason === 'hostkey'
        ? t('gitauth.hostKey', { host })
        : t('gitauth.sshMissing', { host });
  return (
    <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-danger/30 pb-1.5 text-fg">
      <span className="min-w-0 flex-1">{text}</span>
      {problem.protocol === 'https' && problem.host ? (
        <Button size="sm" variant="primary" onClick={onSignIn}>
          {problem.reason === 'rejected' ? t('gitauth.edit') : t('gitauth.signInShort')}
        </Button>
      ) : null}
    </div>
  );
}
