import { useMemo, useState } from 'react';
import { Check, Search } from 'lucide-react';
import { t } from '../i18n';
import { ProviderIcon } from './AgentIcon';

/**
 * Pick a model from the agent's list: search, groups by provider (provider/model),
 * a "default" item. A custom value can be typed and confirmed with Enter.
 */
export function ModelPicker({
  models,
  value,
  onChange,
  defaultLabel,
  loading,
}: {
  models: string[];
  value: string | undefined;
  onChange: (m: string | undefined) => void;
  /** Label of the "default model" item; if unset, the item is hidden. */
  defaultLabel?: string;
  loading?: boolean;
}) {
  const [q, setQ] = useState('');
  const groups = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const list = needle ? models.filter((m) => m.toLowerCase().includes(needle)) : models;
    const map = new Map<string, string[]>();
    for (const m of list) {
      const i = m.indexOf('/');
      const provider = i > 0 ? m.slice(0, i) : '';
      if (!map.has(provider)) map.set(provider, []);
      map.get(provider)!.push(m);
    }
    return [...map.entries()];
  }, [models, q]);
  const total = groups.reduce((n, [, l]) => n + l.length, 0);
  const custom = q.trim() && !models.includes(q.trim()) ? q.trim() : null;

  const item = (m: string | undefined, label: string, sub?: string) => {
    const active = (value ?? '') === (m ?? '');
    return (
      <button
        key={m ?? '__default'}
        type="button"
        onClick={() => onChange(m)}
        className={`flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-sm ${active ? 'bg-accent/12 text-fg' : 'hover:bg-sunken'}`}
      >
        <span className="min-w-0 flex-1 truncate font-mono text-[13px]">{label}</span>
        {sub ? <span className="shrink-0 text-xs text-muted">{sub}</span> : null}
        {active ? <Check className="size-4 shrink-0 text-accent" /> : null}
      </button>
    );
  };

  return (
    <div className="overflow-hidden rounded-xl border border-line">
      <label className="flex items-center gap-2 border-b border-line bg-bg px-3">
        <Search className="size-4 shrink-0 text-muted" />
        <input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              if (custom) onChange(custom);
              else if (total === 1) onChange(groups[0][1][0]);
            }
          }}
          placeholder={t('model.search', { count: models.length })}
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          className="min-w-0 flex-1 bg-transparent py-2.5 text-sm outline-none"
        />
      </label>
      <div className="max-h-72 overflow-y-auto p-1.5">
        {defaultLabel && !q ? item(undefined, defaultLabel) : null}
        {custom ? item(custom, custom, t('model.custom')) : null}
        {loading && !models.length ? <div className="px-2.5 py-2 text-sm text-muted">{t('model.loading')}</div> : null}
        {!loading && !total && !custom ? <div className="px-2.5 py-2 text-sm text-muted">{t('model.none')}</div> : null}
        {groups.map(([provider, list]) => (
          <div key={provider} className="mt-1 first:mt-0">
            {provider && groups.length > 1 ? (
              <div className="sticky top-0 z-10 flex items-center gap-1.5 bg-surface px-2.5 pt-1.5 pb-1 text-xs font-medium text-muted">
                <ProviderIcon name={provider} className="size-3.5" />
                {provider}
                <span className="font-normal">· {list.length}</span>
              </div>
            ) : null}
            {list.map((m) => item(m, provider && groups.length > 1 ? m.slice(provider.length + 1) : m))}
          </div>
        ))}
      </div>
    </div>
  );
}
