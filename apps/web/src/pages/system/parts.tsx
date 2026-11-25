/** Building blocks shared by the System sections. */
import { useEffect, useState, type ReactNode } from 'react';
import { Search } from 'lucide-react';
import { AskButton } from '../../components/AskButton';
import { Sheet, Spinner } from '../../components/ui';
import { t } from '../../i18n';
import { askAboutLogs } from '../../lib/askDraft';

export function fmtUptime(sec: number) {
  const d = Math.floor(sec / 86400);
  const h = Math.floor((sec % 86400) / 3600);
  const m = Math.floor((sec % 3600) / 60);
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`;
}

export function Header({ title, hint, actions }: { title: string; hint?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-4 flex flex-wrap items-end gap-3">
      <div className="min-w-0 flex-1">
        <h2 className="text-lg font-semibold">{title}</h2>
        {hint ? <p className="text-sm text-muted">{hint}</p> : null}
      </div>
      {actions}
    </div>
  );
}

export function Filter({ value, onChange, placeholder }: { value: string; onChange: (v: string) => void; placeholder: string }) {
  return (
    <div className="relative min-w-48 flex-1">
      <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted" />
      <input
        type="search"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="w-full rounded-xl border border-line bg-surface py-2 pr-3 pl-9 text-sm outline-none placeholder:text-muted/70 focus:border-accent"
      />
    </div>
  );
}

export function Chips<T extends string>({ value, onChange, options }: { value: T; onChange: (v: T) => void; options: [T, string][] }) {
  return (
    <div className="flex gap-1 rounded-xl border border-line bg-surface p-1 text-sm">
      {options.map(([id, label]) => (
        <button
          key={id}
          type="button"
          onClick={() => onChange(id)}
          className={`rounded-lg px-2.5 py-1 whitespace-nowrap ${value === id ? 'bg-sunken text-fg' : 'text-muted hover:text-fg'}`}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

export function Bar({ pct }: { pct: number }) {
  const v = Math.min(100, Math.max(0, pct));
  const color = v >= 90 ? 'bg-danger' : v >= 75 ? 'bg-warn' : 'bg-ok';
  return (
    <div className="h-1.5 overflow-hidden rounded-full bg-sunken">
      <div className={`h-full rounded-full ${color}`} style={{ width: `${v}%` }} />
    </div>
  );
}

export const Dot = ({ cls }: { cls: string }) => <span className={`inline-block size-2 shrink-0 rounded-full ${cls}`} />;

export function LogsSheet({ title, subject, load, onClose }: { title: string | null; subject?: string; load: () => Promise<string>; onClose: () => void }) {
  const [logs, setLogs] = useState<string | null>(null);
  useEffect(() => {
    if (!title) return;
    setLogs(null);
    load().then(setLogs, (e) => setLogs(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [title]);
  return (
    <Sheet
      open={title !== null}
      onClose={onClose}
      title={title ?? ''}
      footer={logs && subject ? <AskButton text={askAboutLogs(subject, logs)} variant="primary" label={t('ask.logs')} /> : undefined}
    >
      {logs === null ? (
        <Spinner />
      ) : (
        <pre className="max-h-[65dvh] overflow-auto rounded-lg bg-sunken p-3 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap break-all">
          {logs || t('sys.noLogs')}
        </pre>
      )}
    </Sheet>
  );
}

export function Result({ r }: { r: { ok: boolean; output: string; name: string } | null }) {
  if (!r) return null;
  return (
    <div className={`mb-3 rounded-lg px-3 py-2 text-sm ${r.ok ? 'bg-ok/10 text-ok' : 'border border-danger/40 text-danger'}`}>
      <span className="font-medium">{r.name}:</span> {r.output || (r.ok ? 'OK' : t('git.failed'))}
    </div>
  );
}
