import { useCallback, useEffect, useState, type FormEvent } from 'react';
import { KeyRound, LogOut, Plus, ShieldCheck, Trash2, UserRound } from 'lucide-react';
import type { Role, User } from '@hubness/shared';
import { api } from '../../lib/api';
import { timeAgo } from '../../lib/format';
import { t } from '../../i18n';
import { useAccess, useAuth } from '../../components/AuthGate';
import { AccessSheet } from './AccessSheet';
import { Button, ErrorLine, Field, IconButton, inputCls, Sheet, Spinner } from '../../components/ui';
import { confirmDialog } from '../../components/Confirm';
import { Select } from '../../components/Select';

/** Settings → Users: hub accounts with their roles and permissions (admins), your own sign-in and password. */
export function UsersPage() {
  const auth = useAuth();
  const { admin } = useAccess();
  const [access, setAccess] = useState<User | null>(null);
  const [list, setList] = useState<User[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [changing, setChanging] = useState<User | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // a member sees only its own account
  const load = useCallback(
    () => (admin ? api.users().then(setList, (e) => setError((e as Error).message)) : Promise.resolve(setList(auth.user ? [auth.user] : []))),
    [admin, auth.user],
  );
  useEffect(() => {
    void load();
  }, [load]);

  const remove = async (u: User) => {
    const own = u.id === auth.user?.id;
    const message = own ? t('users.deleteSelfConfirm', { name: u.username }) : t('users.deleteConfirm', { name: u.username });
    if (!(await confirmDialog({ message, confirmLabel: t('common.delete'), danger: true }))) return;
    setError(null);
    try {
      await api.deleteUser(u.id);
      if (own) return window.location.reload();
      await load();
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const open = list !== null && list.length === 0;
  return (
    <div className="space-y-4">
      <p className={`text-sm ${open ? 'text-warn' : 'text-muted'}`}>{open ? t('users.open') : admin ? t('users.intro') : t('users.memberIntro')}</p>
      {auth.user ? (
        <div className="flex items-center gap-3 rounded-xl border border-line bg-surface px-4 py-2.5 text-sm">
          <UserRound className="size-4 shrink-0 text-accent" />
          <span className="min-w-0 flex-1 truncate">{t('auth.signedInAs', { name: auth.user.username })}</span>
          <Button size="sm" variant="ghost" onClick={() => void auth.logout()}>
            <LogOut className="size-3.5" />
            {t('auth.logout')}
          </Button>
        </div>
      ) : null}
      {list === null ? (
        <Spinner />
      ) : list.length ? (
        <ul className="divide-y divide-line overflow-hidden rounded-xl border border-line bg-surface">
          {list.map((u) => (
            <li key={u.id} className="flex items-center gap-3 px-4 py-2.5">
              <UserRound className="size-4 shrink-0 text-accent" />
              <div className="min-w-0 flex-1">
                <div className="truncate font-mono text-sm">
                  {u.username}
                  {u.id === auth.user?.id ? <span className="ml-2 font-sans text-xs text-ok">{t('users.you')}</span> : null}
                  <span className={`ml-2 font-sans text-xs ${u.role === 'admin' ? 'text-accent' : 'text-muted'}`}>
                    {t(u.role === 'admin' ? 'users.role.admin' : 'users.role.member')}
                  </span>
                </div>
                <div className="truncate text-xs text-muted">{t('users.created', { when: timeAgo(u.createdAt) })}</div>
              </div>
              {admin ? (
                <Button size="sm" variant="ghost" onClick={() => setAccess(u)}>
                  <ShieldCheck className="size-3.5" />
                  <span className="hidden sm:inline">{t('users.access')}</span>
                </Button>
              ) : null}
              <Button size="sm" variant="ghost" onClick={() => setChanging(u)}>
                <KeyRound className="size-3.5" />
                <span className="hidden sm:inline">{t('users.changePassword')}</span>
              </Button>
              {admin ? (
                <IconButton
                  label={list.length === 1 ? t('users.lastOne') : t('common.delete')}
                  className="size-8"
                  disabled={list.length === 1}
                  onClick={() => void remove(u)}
                >
                  <Trash2 className="size-3.5" />
                </IconButton>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
      <ErrorLine error={error} />
      {notice ? <p className="text-sm text-ok">{notice}</p> : null}
      {admin ? (
        <Button onClick={() => setAdding(true)}>
          <Plus className="size-4" />
          {t('users.add')}
        </Button>
      ) : null}
      <AddUserSheet
        open={adding}
        first={open}
        onClose={() => setAdding(false)}
        onAdded={async (u) => {
          setAdding(false);
          // the first account turns sign-in on: the gate asks for it right away
          if (!auth.required) return void auth.refresh();
          await load();
          // a new member has no projects yet: choose them right away
          if (u.role === 'member') setAccess(u);
        }}
      />
      <AccessSheet
        user={access}
        onClose={() => setAccess(null)}
        onSaved={async (u) => {
          setAccess(null);
          setNotice(t('users.accessSaved'));
          if (u.id === auth.user?.id) await auth.refresh();
          await load();
        }}
      />
      <PasswordSheet
        user={changing}
        own={changing !== null && changing.id === auth.user?.id}
        onClose={() => setChanging(null)}
        onChanged={() => {
          setChanging(null);
          setNotice(t('users.changed'));
        }}
      />
    </div>
  );
}

function AddUserSheet({ open, first, onClose, onAdded }: { open: boolean; first: boolean; onClose: () => void; onAdded: (u: User) => void }) {
  const [username, setUsername] = useState('');
  const [role, setRole] = useState<Role>('member');
  const [password, setPassword] = useState('');
  const [repeat, setRepeat] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (open) {
      setUsername('');
      setPassword('');
      setRepeat('');
      setError(null);
      setRole('member');
    }
  }, [open]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (password !== repeat) return setError(t('auth.mismatch'));
    setBusy(true);
    setError(null);
    try {
      // the first account is always an admin
      onAdded(await api.createUser({ username: username.trim(), password, role: first ? 'admin' : role }));
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet open={open} onClose={onClose} title={t('users.add')}>
      <form onSubmit={submit} className="space-y-4">
        <Field label={t('auth.username')}>
          <input className={inputCls} autoComplete="off" autoCapitalize="none" spellCheck={false} autoFocus value={username} onChange={(e) => setUsername(e.target.value)} />
        </Field>
        <Field label={t('auth.password')} hint={t('auth.passwordHint')}>
          <input className={inputCls} type="password" autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} />
        </Field>
        <Field label={t('auth.repeat')}>
          <input className={inputCls} type="password" autoComplete="new-password" value={repeat} onChange={(e) => setRepeat(e.target.value)} />
        </Field>
        {first ? null : (
          <Field label={t('users.role')} hint={t(role === 'admin' ? 'users.roleAdminHint' : 'users.roleMemberHint')}>
            <Select
              value={role}
              onChange={setRole}
              options={[
                { value: 'member', label: t('users.role.member') },
                { value: 'admin', label: t('users.role.admin') },
              ]}
            />
          </Field>
        )}
        <ErrorLine error={error} />
        <Button type="submit" variant="primary" loading={busy} disabled={!username.trim() || !password || !repeat}>
          {t('users.add')}
        </Button>
      </form>
    </Sheet>
  );
}

function PasswordSheet({ user, own, onClose, onChanged }: { user: User | null; own: boolean; onClose: () => void; onChanged: () => void }) {
  const [current, setCurrent] = useState('');
  const [password, setPassword] = useState('');
  const [repeat, setRepeat] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setCurrent('');
    setPassword('');
    setRepeat('');
    setError(null);
  }, [user]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!user) return;
    if (password !== repeat) return setError(t('auth.mismatch'));
    setBusy(true);
    setError(null);
    try {
      await api.setUserPassword(user.id, own ? { password, currentPassword: current } : { password });
      onChanged();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet open={user !== null} onClose={onClose} title={user ? t('users.passwordFor', { name: user.username }) : ''}>
      <form onSubmit={submit} className="space-y-4">
        {/* lets password managers tie the new password to the account */}
        <input type="text" autoComplete="username" value={user?.username ?? ''} readOnly hidden />
        {own ? (
          <Field label={t('users.currentPassword')}>
            <input className={inputCls} type="password" autoComplete="current-password" autoFocus value={current} onChange={(e) => setCurrent(e.target.value)} />
          </Field>
        ) : null}
        <Field label={t('users.newPassword')} hint={t('auth.passwordHint')}>
          <input className={inputCls} type="password" autoComplete="new-password" autoFocus={!own} value={password} onChange={(e) => setPassword(e.target.value)} />
        </Field>
        <Field label={t('auth.repeat')}>
          <input className={inputCls} type="password" autoComplete="new-password" value={repeat} onChange={(e) => setRepeat(e.target.value)} />
        </Field>
        <ErrorLine error={error} />
        <Button type="submit" variant="primary" loading={busy} disabled={!password || !repeat || (own && !current)}>
          {t('users.changePassword')}
        </Button>
      </form>
    </Sheet>
  );
}
