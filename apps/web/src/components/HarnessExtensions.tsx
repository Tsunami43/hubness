import { Link } from 'react-router';
import { BookOpen, Bot, ChevronRight, FileText, Plug, Puzzle, SquareSlash } from 'lucide-react';
import type { AgentKind, ExtensionScope } from '@hubness/shared';
import { api } from '../lib/api';
import { useApi } from '../lib/query';
import { t } from '../i18n';
import type { MessageKey } from '../i18n/en';
import { ErrorLine } from './ui';
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

/** The Extensions pages of the Harness section, in menu order. */
export const EXTENSION_PAGES = [
  { to: 'mcp', icon: Plug, label: 'mcp.title', hint: 'mcp.menuHint' },
  { to: 'skills', icon: BookOpen, label: 'skills.title', hint: 'skills.menuHint' },
  { to: 'rules', icon: FileText, label: 'rules.title', hint: 'rules.menuHint' },
  { to: 'subagents', icon: Bot, label: 'subagents.title', hint: 'subagents.menuHint' },
  { to: 'commands', icon: SquareSlash, label: 'commands.title', hint: 'commands.menuHint' },
  { to: 'plugins', icon: Puzzle, label: 'plugins.title', hint: 'plugins.menuHint' },
] as const satisfies readonly { to: string; icon: unknown; label: MessageKey; hint: MessageKey }[];

/**
 * What an agent is extended with, in short: counts with links to the cross-agent Extensions
 * pages, where everything is found, added and removed.
 */
export function ExtensionsSummary({ kind }: { kind: AgentKind }) {
  const ext = useApi(['harness-ext', kind], () => api.harnessExtensions(kind));
  const mcp = useApi(['harness-mcp', ''], () => api.mcp());
  const e = ext.data;
  const servers = mcp.data?.servers.filter((s) => !s.builtIn && s.agents[kind] === 'user').length;
  const counts: Record<(typeof EXTENSION_PAGES)[number]['to'], number | undefined> = {
    mcp: servers,
    skills: e?.skills.length,
    rules: e?.rules.filter((r) => r.exists).length,
    subagents: e?.agents.length,
    commands: e?.commands.length,
    plugins: e?.plugins.length,
  };
  return (
    <section className="rounded-2xl border border-line bg-surface">
      <header className="px-4 pt-4 sm:px-5">
        <h2 className="text-[15px] font-semibold">{t('ext.title')}</h2>
        <p className="text-xs text-muted">{t('ext.summaryHint')}</p>
      </header>
      {ext.error ? (
        <div className="px-4 pt-3 sm:px-5">
          <ErrorLine error={ext.error} />
        </div>
      ) : null}
      <ul className="mt-2 grid gap-px border-t border-line/60 bg-line/40 sm:grid-cols-2">
        {EXTENSION_PAGES.map(({ to, icon: Icon, label }) => (
          <li key={to} className="bg-surface">
            <Link to={`/harness/${to}`} className="flex items-center gap-3 px-4 py-2.5 hover:bg-sunken sm:px-5">
              <Icon className="size-4 shrink-0 text-muted" />
              <span className="min-w-0 flex-1 truncate text-sm">{t(label)}</span>
              <span className="font-mono text-xs text-muted">{counts[to] ?? '…'}</span>
              <ChevronRight className="size-4 shrink-0 text-muted" />
            </Link>
          </li>
        ))}
      </ul>
    </section>
  );
}
