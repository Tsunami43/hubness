import { useEffect, useState, type FormEvent, type ReactNode } from 'react';
import { t } from '../i18n';
import { Button, ErrorLine, Field, inputCls, Spinner } from './ui';

/** Fired by the API client on a 401: the hub wants its access token. */
export const UNAUTHORIZED_EVENT = 'hubness:unauthorized';

type State = 'checking' | 'open' | 'login';

/**
 * Sign-in screen of a hub that requires an access token (HUBNESS_TOKEN). The token is
 * exchanged once for an HttpOnly session cookie, which also covers live streams and preview
 * ports. A hub without a token (or a server without /api/auth) shows the app right away.
 */
export function AuthGate({ children }: { children: ReactNode }) {
  const [state, setState] = useState<State>('checking');
  const [token, setToken] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    fetch('/api/auth/status')
      .then(async (res) => {
        // 404: a server without access tokens
        if (!res.ok) return setState('open');
        const s = (await res.json()) as { required: boolean; authenticated: boolean };
        setState(!s.required || s.authenticated ? 'open' : 'login');
      })
      .catch(() => setState('open'));
    const onUnauthorized = () => setState('login');
    window.addEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
  }, []);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const res = await fetch('/api/auth/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ token: token.trim() }) });
      if (!res.ok) {
        setError(res.status === 401 ? t('auth.wrong') : t('api.error', { status: res.status }));
        return;
      }
      // a clean start: queries and live streams reconnect with the session cookie
      window.location.reload();
    } catch {
      setError(t('auth.unreachable'));
    } finally {
      setBusy(false);
    }
  };

  if (state === 'checking')
    return (
      <div className="flex h-dvh items-center justify-center bg-bg text-muted">
        <Spinner />
      </div>
    );
  if (state === 'open') return <>{children}</>;
  return (
    <div className="flex h-dvh items-center justify-center bg-bg px-4">
      <form onSubmit={submit} className="w-full max-w-sm space-y-4 border border-line bg-surface p-5 shadow-2xl shadow-black/50">
        <div>
          <div className="text-base font-semibold">{t('auth.title')}</div>
          <div className="mt-1 text-sm text-muted">{t('auth.hint')}</div>
        </div>
        <Field label={t('auth.token')}>
          <input className={inputCls} type="password" autoComplete="current-password" autoFocus value={token} onChange={(e) => setToken(e.target.value)} />
        </Field>
        <ErrorLine error={error} />
        <Button type="submit" variant="primary" className="w-full" loading={busy} disabled={!token.trim()}>
          {t('auth.signIn')}
        </Button>
      </form>
    </div>
  );
}
