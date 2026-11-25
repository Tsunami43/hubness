import { useState, type ReactNode } from 'react';
import { FileText, Pencil, Plus } from 'lucide-react';
import type { AgentKind, ExtensionScope, HarnessExtensions, RulesFileInfo } from '@hubness/shared';
import { api } from '../lib/api';
import { useApi } from '../lib/query';
import { t } from '../i18n';
import { Button, ErrorLine, Spinner } from './ui';
import { Select } from './Select';

/** Pick a project whose own config / folders are shown too (empty — the user's only). */
export function ProjectSelect({ value, onChange }: { value: string; onChange: (id: string) => void }) {
  const { data } = useApi(['projects'], () => api.projects());
  const options = [{ value: '', label: t('ext.noProject') }, ...(data ?? []).filter((p) => p.kind === 'normal').map((p) => ({ value: p.id, label: p.name }))];
  return <Select size="sm" value={value} options={options} onChange={onChange} label={t('ext.project')} className="w-48 max-w-full" />;
}

export function ScopeBadge({ scope }: { scope: ExtensionScope | 'builtin' }) {
  const cls = scope === 'project' ? 'border-accent/40 text-accent' : scope === 'plugin' ? 'border-warn/40 text-warn' : 'border-line text-muted';
  return <span className={`shrink-0 rounded-full border px-1.5 py-px text-[11px] ${cls}`}>{t(`ext.scope.${scope}`)}</span>;
}

type Tab = 'skills' | 'rules' | 'agents' | 'commands' | 'plugins';

/**
 * What an agent is extended with: skills, rules files (editable), subagents, commands and
 * plugins, read from its own folders and, with a project picked, the project's.
 */
export function ExtensionsCard({ kind }: { kind: AgentKind }) {
  const [project, setProject] = useState('');
  const [tab, setTab] = useState<Tab>('skills');
  const { data, loading, error, reload } = useApi(['harness-ext', kind, project], () => api.harnessExtensions(kind, project || undefined));
  const counts: Record<Tab, number> = {
    skills: data?.skills.length ?? 0,
    rules: data?.rules.filter((r) => r.exists).length ?? 0,
    agents: data?.agents.length ?? 0,
    commands: data?.commands.length ?? 0,
    plugins: data?.plugins.length ?? 0,
  };
  const tabs: Tab[] = ['skills', 'rules', 'agents', 'commands', ...(counts.plugins ? (['plugins'] as Tab[]) : [])];
  return (
    <section className="overflow-hidden rounded-2xl border border-line bg-surface">
      <header className="flex flex-wrap items-center gap-2 px-4 pt-4 sm:px-5">
        <h2 className="min-w-0 flex-1 text-[15px] font-semibold">{t('ext.title')}</h2>
        <ProjectSelect value={project} onChange={setProject} />
      </header>
      <div role="tablist" className="flex gap-1 overflow-x-auto border-b border-line/60 px-3 pt-3 sm:px-4">
        {tabs.map((id) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            onClick={() => setTab(id)}
            className={`-mb-px flex shrink-0 items-center gap-1.5 border-b-2 px-2.5 py-2 text-sm ${tab === id ? 'border-accent text-fg' : 'border-transparent text-muted hover:text-fg'}`}
          >
            {t(`ext.tab.${id}`)}
            <span className="font-mono text-[11px] text-muted">{counts[id]}</span>
          </button>
        ))}
      </div>
      <div className="px-4 py-4 sm:px-5">
        {error ? <ErrorLine error={error} /> : null}
        {!data && loading ? (
          <div className="flex justify-center py-6">
            <Spinner />
          </div>
        ) : data ? (
          <TabBody tab={tab} data={data} kind={kind} project={project} onSaved={reload} />
        ) : null}
      </div>
    </section>
  );
}

function TabBody({ tab, data, kind, project, onSaved }: { tab: Tab; data: HarnessExtensions; kind: AgentKind; project: string; onSaved: () => Promise<void> }) {
  if (tab === 'rules')
    return (
      <ul className="divide-y divide-line/50 rounded-xl border border-line/60">
        {data.rules.map((r) => (
          <RulesRow key={r.path} r={r} kind={kind} project={project} onSaved={onSaved} />
        ))}
      </ul>
    );
  const items: { key: string; name: string; scope: ExtensionScope; description: string; path: string; aside?: string | null }[] =
    tab === 'skills'
      ? data.skills.map((s) => ({ key: s.path, ...s }))
      : tab === 'agents'
        ? data.agents.map((a) => ({ key: `${a.path}#${a.name}`, ...a, aside: a.model }))
        : tab === 'commands'
          ? data.commands.map((c) => ({ key: `${c.path}#${c.name}`, ...c, name: `/${c.name}` }))
          : data.plugins.map((p) => ({
              key: `${p.path}#${p.name}`,
              name: p.name,
              scope: p.scope,
              description: p.enabled ? t('ext.enabled') : t('ext.disabled'),
              path: p.path,
              aside: p.version,
            }));
  if (!items.length) return <p className="text-sm text-muted">{t(`ext.empty.${tab}`)}</p>;
  return (
    <ul className="divide-y divide-line/50 rounded-xl border border-line/60">
      {items.map((it) => (
        <li key={it.key} className="px-3.5 py-2.5">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className="text-sm font-medium break-all">{it.name}</span>
            <ScopeBadge scope={it.scope} />
            {it.aside ? <span className="font-mono text-[11px] text-muted">{it.aside}</span> : null}
          </div>
          {it.description ? <p className="mt-0.5 line-clamp-3 text-xs whitespace-pre-line text-muted">{it.description}</p> : null}
          <div className="mt-0.5 truncate font-mono text-[11px] text-muted/80" title={it.path}>
            {it.path}
          </div>
        </li>
      ))}
    </ul>
  );
}

const fmtSize = (n: number) => (n < 1024 ? `${n} B` : `${(n / 1024).toFixed(1)} KB`);

/** A rules file: preview, and an editor that saves it back (only files the server listed). */
function RulesRow({ r, kind, project, onSaved }: { r: RulesFileInfo; kind: AgentKind; project: string; onSaved: () => Promise<void> }) {
  const [text, setText] = useState<string | null>(null);
  const [original, setOriginal] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const open = async () => {
    setError(null);
    setBusy(true);
    try {
      const f = await api.rulesFile(kind, r.path, project || undefined);
      setText(f.content);
      setOriginal(f.content);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const save = async () => {
    if (text === null) return;
    setError(null);
    setBusy(true);
    try {
      await api.saveRulesFile(kind, r.path, text, project || undefined);
      setText(null);
      await onSaved();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  let body: ReactNode = null;
  if (text !== null)
    body = (
      <div className="mt-2 space-y-2">
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          spellCheck={false}
          rows={Math.min(24, Math.max(8, text.split('\n').length + 1))}
          className="w-full resize-y rounded-lg border border-line bg-bg px-3 py-2 font-mono text-[13px] outline-none focus:border-accent"
          aria-label={r.path}
        />
        <div className="flex items-center gap-2">
          <Button size="sm" variant="primary" loading={busy} disabled={text === original && r.exists} onClick={save}>
            {t('common.save')}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setText(null)}>
            {t('common.cancel')}
          </Button>
        </div>
      </div>
    );
  else if (r.preview) body = <pre className="mt-1.5 line-clamp-4 font-mono text-xs whitespace-pre-wrap text-muted">{r.preview}</pre>;
  return (
    <li className="px-3.5 py-2.5">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <FileText className="size-4 shrink-0 text-muted" />
        <span className="text-sm font-medium">{r.label}</span>
        <ScopeBadge scope={r.scope} />
        <span className="text-xs text-muted">{r.exists ? fmtSize(r.size) : t('ext.missing')}</span>
        {text === null ? (
          <button
            type="button"
            onClick={() => void open()}
            disabled={busy}
            className="ml-auto inline-flex items-center gap-1 rounded-md border border-line px-2 py-1 text-xs text-muted hover:bg-sunken hover:text-fg"
          >
            {r.exists ? <Pencil className="size-3.5" /> : <Plus className="size-3.5" />}
            {r.exists ? t('ext.edit') : t('ext.create')}
          </button>
        ) : null}
      </div>
      <div className="mt-0.5 truncate font-mono text-[11px] text-muted/80" title={r.path}>
        {r.path}
      </div>
      {body}
      {error ? (
        <div className="mt-2">
          <ErrorLine error={error} />
        </div>
      ) : null}
    </li>
  );
}
