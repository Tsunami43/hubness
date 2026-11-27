import { useState, type FormEvent } from 'react';
import { LogOut, UserRound } from 'lucide-react';
import { api } from '../../lib/api';
import { t } from '../../i18n';
import { useAuth } from '../../components/AuthGate';
import { Button, ErrorLine, Field, inputCls } from '../../components/ui';

/**
 * Settings → Account: the hub's one account (hubness is a one-person hub). Shows who is signed
 * in, changes the password and logs out; a hub without an account can create it here.
 */
export function AccountPage() {
  const auth = useAuth();
  if (!auth.user) return <CreateAccount onCreated={() => window.location.reload()} />;
  return (
    <div className="space-y-4">
      <p className="text-sm text-muted">{t('account.intro')}</p>
      <div className="flex items-center gap-3 rounded-xl border border-line bg-surface px-4 py-2.5 text-sm">
        <UserRound className="size-4 shrink-0 text-accent" />
        <span className="min-w-0 flex-1 truncate">{t('auth.signedInAs', { name: auth.user.username })}</span>
        <Button size="sm" variant="secondary" onClick={() => void auth.logout()}>
          <LogOut className="size-3.5" />
          {t('auth.logout')}
        </Button>
      </div>
      <ChangePassword username={auth.user.username} />
    </div>
  );
}

function ChangePassword({ username }: { username: string }) {
  const [current, setCurrent] = useState('');
  const [password, setPassword] = useState('');
  const [repeat, setRepeat] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (password !== repeat) return setError(t('auth.mismatch'));
    setBusy(true);
    setError(null);
    setDone(false);
    try {
      await api.changePassword({ currentPassword: current, password });
      setCurrent('');
      setPassword('');
      setRepeat('');
      setDone(true);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={submit} className="space-y-3 rounded-xl border border-line bg-surface p-4">
      <h3 className="text-sm font-semibold">{t('account.changePassword')}</h3>
      {/* lets password managers tie the new password to the account */}
      <input type="text" autoComplete="username" value={username} readOnly hidden />
      <Field label={t('account.currentPassword')}>
        <input className={inputCls} type="password" autoComplete="current-password" value={current} onChange={(e) => setCurrent(e.target.value)} />
      </Field>
      <Field label={t('account.newPassword')} hint={t('auth.passwordHint')}>
        <input className={inputCls} type="password" autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} />
      </Field>
      <Field label={t('auth.repeat')}>
        <input className={inputCls} type="password" autoComplete="new-password" value={repeat} onChange={(e) => setRepeat(e.target.value)} />
      </Field>
      <ErrorLine error={error} />
      {done ? <p className="text-sm text-ok">{t('account.changed')}</p> : null}
      <Button type="submit" variant="primary" loading={busy} disabled={!current || !password || !repeat}>
        {t('account.changePassword')}
      </Button>
    </form>
  );
}

/** No account yet: the hub is open to this machine; creating the account turns sign-in on. */
function CreateAccount({ onCreated }: { onCreated: () => void }) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [repeat, setRepeat] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (password !== repeat) return setError(t('auth.mismatch'));
    setBusy(true);
    setError(null);
    try {
      const res = await fetch('/api/auth/setup', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ username: username.trim(), password }),
      });
      if (!res.ok) {
        const data = (await res.json().catch(() => null)) as { error?: string } | null;
        setError(data?.error ?? t('api.error', { status: res.status }));
        return;
      }
      onCreated();
    } catch {
      setError(t('auth.unreachable'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <p className="text-sm text-warn">{t('account.open')}</p>
      <form onSubmit={submit} className="space-y-3 rounded-xl border border-line bg-surface p-4">
        <h3 className="text-sm font-semibold">{t('account.create')}</h3>
        <Field label={t('auth.username')}>
          <input className={inputCls} autoComplete="username" autoCapitalize="none" spellCheck={false} value={username} onChange={(e) => setUsername(e.target.value)} />
        </Field>
        <Field label={t('auth.password')} hint={t('auth.passwordHint')}>
          <input className={inputCls} type="password" autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} />
        </Field>
        <Field label={t('auth.repeat')}>
          <input className={inputCls} type="password" autoComplete="new-password" value={repeat} onChange={(e) => setRepeat(e.target.value)} />
        </Field>
        <ErrorLine error={error} />
        <Button type="submit" variant="primary" loading={busy} disabled={!username.trim() || !password || !repeat}>
          {t('auth.createAccount')}
        </Button>
      </form>
    </div>
  );
}
