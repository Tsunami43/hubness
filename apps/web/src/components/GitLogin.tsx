import { useEffect, useState } from 'react';
import { ExternalLink } from 'lucide-react';
import type { GitAuthProblem, GitCredentialInfo, GitCredentialKind } from '@hubness/shared';
import { api } from '../lib/api';
import { t } from '../i18n';
import { Button, ErrorLine, Field, inputCls, Sheet } from './ui';

/** The host part of what was typed: `https://gitlab.example.com/group/repo` → `gitlab.example.com`. */
export function hostOf(input: string): string {
  return input
    .trim()
    .toLowerCase()
    .replace(/^[a-z+]+:\/\//, '')
    .replace(/^[^@/]*@/, '')
    .replace(/[/?#].*$/, '')
    // scp-like `git@host:group/repo` (a `:port` stays)
    .replace(/:(?!\d+$).*$/, '');
}

type Hosting = 'github' | 'gitlab' | 'gitea' | 'bitbucket';

/** Token pages, opened with the scopes git needs already filled in where the hosting allows it. */
const TOKEN_PAGES: Record<Hosting, (host: string) => string> = {
  // a classic token: fine-grained ones can't be prefilled; `workflow` lets pushes touch CI files
  github: (h) => `https://${h}/settings/tokens/new?scopes=repo,workflow&description=hubness`,
  gitlab: (h) => `https://${h}/-/user_settings/personal_access_tokens?name=hubness&scopes=read_repository,write_repository`,
  gitea: (h) => `https://${h}/user/settings/applications`,
  bitbucket: () => 'https://id.atlassian.com/manage-profile/security/api-tokens',
};

function hostingOf(host: string): Hosting | null {
  if (host === 'github.com' || host.startsWith('github.')) return 'github';
  if (host === 'gitlab.com' || /(^|[.-])gitlab([.-]|$)/.test(host)) return 'gitlab';
  if (host === 'codeberg.org' || /(^|[.-])(gitea|forgejo)([.-]|$)/.test(host)) return 'gitea';
  if (host === 'bitbucket.org') return 'bitbucket';
  return null;
}

/** Where to create a token for `host`: one page for a known hosting, the likely ones for another domain. */
export function tokenPages(host: string): { label: string | null; url: string }[] {
  if (!/^[a-z0-9.-]+\.[a-z]{2,}(:\d+)?$/.test(host)) return [];
  const known = hostingOf(host);
  if (known) return [{ label: null, url: TOKEN_PAGES[known](host) }];
  // self-hosted on a custom domain: most often GitLab, else Gitea / Forgejo
  return [
    { label: 'GitLab', url: TOKEN_PAGES.gitlab(host) },
    { label: 'Gitea / Forgejo', url: TOKEN_PAGES.gitea(host) },
  ];
}

/** Public hostings offered under the host field. */
const POPULAR_HOSTS = ['github.com', 'gitlab.com', 'codeberg.org', 'bitbucket.org'];

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
  // hosts of the projects' remotes first (the ones likely to need a login), then public hostings
  const [suggested, setSuggested] = useState<string[]>(POPULAR_HOSTS);

  useEffect(() => {
    if (!open || fixedHost) return;
    let live = true;
    Promise.all([api.projects().catch(() => []), api.gitCredentials().catch(() => [])]).then(([projects, creds]) => {
      if (!live) return;
      const stored = new Set(creds.map((c) => c.host));
      const fromProjects = projects.map((p) => (p.repoUrl ? hostOf(p.repoUrl) : '')).filter(Boolean);
      setSuggested([...new Set([...fromProjects, ...POPULAR_HOSTS])].filter((h) => !stored.has(h)).slice(0, 8));
    });
    return () => {
      live = false;
    };
  }, [open, fixedHost]);

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

  const h = hostOf(fixedHost ?? host);
  const pages = tokenPages(h);
  const ready = h && username.trim() && secret;

  const save = async () => {
    if (!ready) return;
    setBusy(true);
    setError(null);
    try {
      onSaved(await api.setGitCredential(h, { username: username.trim(), kind, secret: kind === 'token' ? secret.trim() : secret }));
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
        {fixedHost ? null : suggested.length ? (
          <div className="-mt-2 flex flex-wrap gap-1.5">
            {suggested.map((s) => (
              <button
                key={s}
                type="button"
                onClick={() => setHost(s)}
                aria-pressed={hostOf(host) === s}
                className={`border px-2 py-0.5 font-mono text-xs ${hostOf(host) === s ? 'border-accent text-accent' : 'border-line text-muted hover:text-fg'}`}
              >
                {s}
              </button>
            ))}
          </div>
        ) : null}
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
        {pages.length ? (
          // a token works with two-factor auth: the page opens with the scopes filled in
          <div className="space-y-1.5 border border-accent/30 bg-accent/5 px-3 py-2.5">
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
              {pages.map((p) => (
                <a
                  key={p.url}
                  href={p.url}
                  target="_blank"
                  rel="noreferrer"
                  onClick={() => setKind('token')}
                  className="inline-flex items-center gap-1 text-sm text-accent hover:underline"
                >
                  {p.label ? t('gitauth.createTokenAs', { kind: p.label }) : t('gitauth.createToken', { host: h })}
                  <ExternalLink className="size-3.5" />
                </a>
              ))}
            </div>
            <p className="text-xs text-muted">{pages.length > 1 ? t('gitauth.createTokenUnknown') : t('gitauth.createTokenHint')}</p>
          </div>
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
