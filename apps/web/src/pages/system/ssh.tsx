import { useState } from 'react';
import { Check, Copy, KeyRound, Pencil, Plug, Plus, ShieldCheck, Trash2, X } from 'lucide-react';
import type { SshConnection } from '@hubness/shared';
import { api } from '../../lib/api';
import { useApi } from '../../lib/query';
import { Button, Empty, ErrorLine, Field, IconButton, inputCls, Sheet, Spinner } from '../../components/ui';
import { t } from '../../i18n';
import { Header } from './parts';
import { copyText } from '../../lib/clipboard';

function CopyButton({ text, label }: { text: string; label: string }) {
  const [state, setState] = useState<'idle' | 'done' | 'failed'>('idle');
  return (
    <Button
      size="sm"
      onClick={() =>
        void copyText(text).then((ok) => {
          setState(ok ? 'done' : 'failed');
          setTimeout(() => setState('idle'), ok ? 1500 : 4000);
        })
      }
    >
      {state === 'done' ? <Check className="size-3.5 text-ok" /> : state === 'failed' ? <X className="size-3.5 text-danger" /> : <Copy className="size-3.5" />}
      {state === 'failed' ? t('common.copyFailed') : label}
    </Button>
  );
}

/** System → SSH: the hub's SSH connections (keys it holds), shared by every project. */
export function SshSection() {
  const { data, reload } = useApi(['ssh'], () => api.sshConnections());
  const [editing, setEditing] = useState<SshConnection | 'new' | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<{ id: string; ok: boolean; output: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const test = async (c: SshConnection) => {
    setBusy(c.id);
    setError(null);
    try {
      const r = await api.testSsh(c.id);
      setResult({ id: c.id, ok: r.ok, output: r.output });
      await reload();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };
  const remove = async (c: SshConnection) => {
    if (!confirm(t('ssh.deleteConfirm', { name: c.name }))) return;
    try {
      await api.deleteSsh(c.id);
      await reload();
    } catch (e) {
      setError((e as Error).message);
    }
  };

  return (
    <div>
      <Header
        title="SSH"
        hint={t('ssh.hint')}
        actions={
          <Button variant="primary" onClick={() => setEditing('new')}>
            <Plus className="size-4" />
            {t('ssh.new')}
          </Button>
        }
      />
      <ErrorLine error={error} />
      {!data ? (
        <Spinner />
      ) : !data.length ? (
        <Empty icon={<KeyRound className="size-8" />} title={t('ssh.empty')} hint={t('ssh.emptyHint')} />
      ) : (
        <ul className="space-y-2">
          {data.map((c) => (
            <li key={c.id} className="rounded-xl border border-line p-3">
              <div className="flex flex-wrap items-center gap-2">
                <KeyRound className="size-4 shrink-0 text-accent" />
                <span className="font-mono font-medium">{c.name}</span>
                <span className="min-w-0 truncate font-mono text-xs text-muted">
                  {c.username}@{c.host}
                  {c.port !== 22 ? `:${c.port}` : ''}
                </span>
                {c.hostKeys ? (
                  <span title={t('ssh.pinned')} className="text-ok">
                    <ShieldCheck className="size-4" />
                  </span>
                ) : (
                  <span className="rounded-md bg-warn/15 px-1.5 py-0.5 text-[11px] text-warn">{t('ssh.notTested')}</span>
                )}
                <span className="flex-1" />
                {c.hostKeys ? (
                  <Button size="sm" loading={busy === c.id} onClick={() => void test(c)}>
                    <Plug className="size-3.5" />
                    {t('ssh.test')}
                  </Button>
                ) : null}
                <IconButton label={t('ssh.edit')} className="size-8" onClick={() => setEditing(c)}>
                  <Pencil className="size-3.5" />
                </IconButton>
                <IconButton label={t('common.delete')} className="size-8" onClick={() => void remove(c)}>
                  <Trash2 className="size-3.5" />
                </IconButton>
              </div>
              {c.description ? <p className="mt-1.5 text-sm text-muted">{c.description}</p> : null}
              {result?.id === c.id ? (
                <pre className={`mt-2 rounded-lg px-3 py-2 text-xs whitespace-pre-wrap break-all ${result.ok ? 'bg-ok/10 text-ok' : 'border border-danger/40 text-danger'}`}>
                  {result.ok ? t('ssh.connected') : result.output || t('git.failed')}
                </pre>
              ) : null}
              {/* not tested yet: the two steps that make it usable, in the open; after that the key folds away */}
              {c.hostKeys ? <PublicKey c={c} /> : <Setup c={c} busy={busy === c.id} onTest={() => void test(c)} />}
            </li>
          ))}
        </ul>
      )}
      {editing ? (
        <SshForm
          initial={editing === 'new' ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            void reload();
          }}
        />
      ) : null}
    </div>
  );
}

const installLine = (c: SshConnection) => `mkdir -p ~/.ssh && echo '${c.publicKey}' >> ~/.ssh/authorized_keys`;

/** A new connection: put the key on the server, then test — agents get it only after that. */
function Setup({ c, busy, onTest }: { c: SshConnection; busy: boolean; onTest: () => void }) {
  const line = installLine(c);
  return (
    <ol className="mt-3 space-y-3 rounded-lg bg-sunken/60 p-3 text-sm">
      <li className="space-y-1.5">
        <div>
          <span className="mr-1.5 font-mono text-muted">1</span>
          {t('ssh.step1', { user: c.username })}
        </div>
        <pre className="rounded-lg bg-sunken p-2 font-mono text-[11.5px] whitespace-pre-wrap break-all">{line}</pre>
        <div className="flex flex-wrap gap-2">
          <CopyButton text={line} label={t('ssh.copyCommand')} />
          <CopyButton text={c.publicKey} label={t('ssh.copyKey')} />
        </div>
      </li>
      <li className="flex flex-wrap items-center gap-2">
        <span>
          <span className="mr-1.5 font-mono text-muted">2</span>
          {t('ssh.step2')}
        </span>
        <Button size="sm" variant="primary" loading={busy} onClick={onTest}>
          <Plug className="size-3.5" />
          {t('ssh.test')}
        </Button>
      </li>
    </ol>
  );
}

function PublicKey({ c }: { c: SshConnection }) {
  const [open, setOpen] = useState(false);
  const line = installLine(c);
  return (
    <div className="mt-2">
      <button type="button" className="text-xs text-muted hover:text-fg" onClick={() => setOpen(!open)}>
        {open ? '▾' : '▸'} {t('ssh.publicKey')}
      </button>
      {open ? (
        <div className="mt-1.5 space-y-2">
          <pre className="rounded-lg bg-sunken p-2 font-mono text-[11.5px] whitespace-pre-wrap break-all">{c.publicKey}</pre>
          <p className="text-xs text-muted">{t('ssh.installHint')}</p>
          <pre className="rounded-lg bg-sunken p-2 font-mono text-[11.5px] whitespace-pre-wrap break-all">{line}</pre>
          <div className="flex flex-wrap gap-2">
            <CopyButton text={c.publicKey} label={t('ssh.copyKey')} />
            <CopyButton text={line} label={t('ssh.copyCommand')} />
          </div>
        </div>
      ) : null}
    </div>
  );
}

function SshForm({ initial, onClose, onSaved }: { initial: SshConnection | null; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState(initial?.name ?? '');
  const [host, setHost] = useState(initial?.host ?? '');
  const [port, setPort] = useState(String(initial?.port ?? 22));
  const [username, setUsername] = useState(initial?.username ?? 'root');
  const [description, setDescription] = useState(initial?.description ?? '');
  const [importKey, setImportKey] = useState(false);
  const [privateKey, setPrivateKey] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async () => {
    setBusy(true);
    setError(null);
    const body = { name: name.trim(), host: host.trim(), port: Number(port) || 22, username: username.trim(), description };
    try {
      if (initial) await api.updateSsh(initial.id, body);
      else await api.createSsh({ ...body, ...(importKey && privateKey.trim() ? { privateKey } : {}) });
      onSaved();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet
      open
      onClose={onClose}
      title={initial ? initial.name : t('ssh.new')}
      footer={
        <>
          <span className="flex-1" />
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button variant="primary" loading={busy} disabled={!name.trim() || !host.trim() || !username.trim() || (importKey && !privateKey.trim())} onClick={() => void save()}>
            {initial ? t('common.save') : importKey ? t('ssh.import') : t('ssh.generate')}
          </Button>
        </>
      }
    >
      <div className="space-y-3">
        <Field label={t('ssh.name')} hint={t('ssh.nameHint')}>
          <input className={`${inputCls} font-mono`} value={name} onChange={(e) => setName(e.target.value.replace(/[^A-Za-z0-9._-]/g, '-'))} placeholder="prod-api" autoCapitalize="off" spellCheck={false} autoFocus />
        </Field>
        <div className="grid grid-cols-[1fr_6rem] gap-2">
          <Field label={t('ssh.host')}>
            <input className={`${inputCls} font-mono`} value={host} onChange={(e) => setHost(e.target.value)} placeholder="203.0.113.10" autoCapitalize="off" spellCheck={false} />
          </Field>
          <Field label={t('ssh.port')}>
            <input className={`${inputCls} font-mono`} value={port} inputMode="numeric" onChange={(e) => setPort(e.target.value.replace(/\D/g, ''))} />
          </Field>
        </div>
        <Field label={t('ssh.user')}>
          <input className={`${inputCls} font-mono`} value={username} onChange={(e) => setUsername(e.target.value)} autoCapitalize="off" spellCheck={false} />
        </Field>
        <Field label={t('ssh.description')} hint={t('ssh.descriptionHint')}>
          <textarea className={`${inputCls} min-h-24`} value={description} onChange={(e) => setDescription(e.target.value)} placeholder={t('ssh.descriptionPlaceholder')} />
        </Field>
        {!initial ? (
          <div className="space-y-2 rounded-xl border border-line p-3">
            <div className="flex gap-1 text-sm">
              <button type="button" onClick={() => setImportKey(false)} className={`rounded-lg px-2.5 py-1 ${!importKey ? 'bg-sunken text-fg' : 'text-muted hover:text-fg'}`}>
                {t('ssh.keyGenerate')}
              </button>
              <button type="button" onClick={() => setImportKey(true)} className={`rounded-lg px-2.5 py-1 ${importKey ? 'bg-sunken text-fg' : 'text-muted hover:text-fg'}`}>
                {t('ssh.keyImport')}
              </button>
            </div>
            {importKey ? (
              <textarea
                className={`${inputCls} min-h-28 font-mono text-xs`}
                value={privateKey}
                onChange={(e) => setPrivateKey(e.target.value)}
                placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
                spellCheck={false}
              />
            ) : (
              <p className="text-xs text-muted">{t('ssh.keyGenerateHint')}</p>
            )}
          </div>
        ) : null}
        <ErrorLine error={error} />
      </div>
    </Sheet>
  );
}
