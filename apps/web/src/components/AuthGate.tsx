import { createContext, useCallback, useContext, useEffect, useState, type FormEvent, type ReactNode } from 'react';
import type { AuthStatus, User } from '@hubness/shared';
import { t } from '../i18n';
import { Button, ErrorLine, Field, inputCls, Spinner } from './ui';

/** Fired by the API client on a 401: the session is gone, sign in again. */
export const UNAUTHORIZED_EVENT = 'hubness:unauthorized';

/** The first-account form was skipped on this browser (the hub stays open to its own machine). */
const SKIP_SETUP_KEY = 'hubness.skipSetup';

type State = 'checking' | 'open' | 'login' | 'setup';

interface Auth {
  /** The signed-in user; `null` on a hub without accounts. */
  user: User | null;
  /** The hub has accounts (sign-in is on). */
  required: boolean;
  logout: () => Promise<void>;
  /** Re-read the status (after the accounts changed). */
  refresh: () => Promise<void>;
}

const AuthContext = createContext<Auth>({ user: null, required: false, logout: async () => {}, refresh: async () => {} });

export const useAuth = () => useContext(AuthContext);

const skipped = () => {
  try {
    return localStorage.getItem(SKIP_SETUP_KEY) === '1';
  } catch {
    return false;
  }
};

/**
 * Sign-in for a hub with accounts. The session lives in an HttpOnly cookie, which also covers
 * live streams and preview ports. A hub without accounts shows a form for the first one
 * (it can be skipped: the hub then stays open to its own machine only); a server without
 * /api/auth shows the app right away.
 */
export function AuthGate({ children }: { children: ReactNode }) {
  const [state, setState] = useState<State>('checking');
  const [status, setStatus] = useState<AuthStatus | null>(null);

  const refresh = useCallback(async () => {
    try {
      const res = await fetch('/api/auth/status');
      // 404: a server without accounts
      if (!res.ok) return setState('open');
      const s = (await res.json()) as AuthStatus;
      setStatus(s);
      if (s.user || (!s.required && !s.setup)) setState('open');
      else if (s.setup) setState(skipped() ? 'open' : 'setup');
      else setState('login');
    } catch {
      setState('open');
    }
  }, []);

  useEffect(() => {
    void refresh();
    const onUnauthorized = () => {
      // accounts may have just been turned on: ask the hub what it wants
      try {
        localStorage.removeItem(SKIP_SETUP_KEY);
      } catch {
        /* storage unavailable */
      }
      void refresh();
    };
    window.addEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
  }, [refresh]);

  const logout = useCallback(async () => {
    await fetch('/api/auth/logout', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' }).catch(() => null);
    // a clean start: queries and live streams are dropped with the session
    window.location.reload();
  }, []);

  const skip = () => {
    try {
      localStorage.setItem(SKIP_SETUP_KEY, '1');
    } catch {
      /* storage unavailable: skip for this page only */
    }
    setState('open');
  };

  if (state === 'checking')
    return (
      <div className="flex h-dvh items-center justify-center bg-bg text-muted">
        <Spinner />
      </div>
    );
  if (state === 'open')
    return <AuthContext.Provider value={{ user: status?.user ?? null, required: status?.required ?? false, logout, refresh }}>{children}</AuthContext.Provider>;
  return <SignInForm setup={state === 'setup'} onSkip={skip} />;
}

/** Username + password; for the first account (`setup`) the password is typed twice. */
function SignInForm({ setup, onSkip }: { setup: boolean; onSkip: () => void }) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [repeat, setRepeat] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (setup && password !== repeat) return setError(t('auth.mismatch'));
    setBusy(true);
    setError(null);
    try {
      const res = await fetch(setup ? '/api/auth/setup' : '/api/auth/login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ username: username.trim(), password }),
      });
      if (!res.ok) {
        const data = (await res.json().catch(() => null)) as { error?: string } | null;
        setError(res.status === 401 ? t('auth.wrong') : (data?.error ?? t('api.error', { status: res.status })));
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

  const ready = username.trim() && password && (!setup || repeat);
  return (
    <div className="flex h-dvh items-center justify-center bg-bg px-4">
      <form onSubmit={submit} className="w-full max-w-sm space-y-4 border border-line bg-surface p-5 shadow-2xl shadow-black/50">
        <div>
          <div className="text-base font-semibold">{setup ? t('auth.setupTitle') : t('auth.title')}</div>
          <div className="mt-1 text-sm text-muted">{setup ? t('auth.setupHint') : t('auth.hint')}</div>
        </div>
        <Field label={t('auth.username')}>
          <input
            className={inputCls}
            autoComplete="username"
            autoCapitalize="none"
            spellCheck={false}
            autoFocus
            value={username}
            onChange={(e) => setUsername(e.target.value)}
          />
        </Field>
        <Field label={t('auth.password')} hint={setup ? t('auth.passwordHint') : undefined}>
          <input
            className={inputCls}
            type="password"
            autoComplete={setup ? 'new-password' : 'current-password'}
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </Field>
        {setup ? (
          <Field label={t('auth.repeat')}>
            <input className={inputCls} type="password" autoComplete="new-password" value={repeat} onChange={(e) => setRepeat(e.target.value)} />
          </Field>
        ) : null}
        <ErrorLine error={error} />
        <Button type="submit" variant="primary" className="w-full" loading={busy} disabled={!ready}>
          {setup ? t('auth.createAccount') : t('auth.signIn')}
        </Button>
        {setup ? (
          <button type="button" onClick={onSkip} className="w-full text-center text-xs text-muted hover:text-fg">
            {t('auth.skip')}
          </button>
        ) : null}
      </form>
    </div>
  );
}
