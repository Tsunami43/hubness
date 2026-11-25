import { useState } from 'react';
import { Pencil, Plus } from 'lucide-react';
import type { AgentKind, ExtensionScope, HarnessExtensions, RulesFileInfo } from '@hubness/shared';
import { api } from '../../lib/api';
import { useApi } from '../../lib/query';
import { useAgents } from '../../lib/agents';
import { ScopeBadge } from '../../components/HarnessExtensions';
import { Button, ErrorLine, Sheet, Spinner } from '../../components/ui';
import { AgentHeads, EmptyRow, ExtPage, FileViewer, errText, matches, useProjectParam } from '../../components/harness/kit';
import { t } from '../../i18n';
import type { MessageKey } from '../../i18n/en';

/**
 * Harness → Extensions → Rules, Subagents, Commands, Plugins: what every agent reads from
 * its folders, as a matrix "item × agent" (GET /harness/extensions). Rules files open in an
 * editor; subagents and commands open read-only.
 */

export type ListKind = 'rules' | 'subagents' | 'commands' | 'plugins';

const HELP: Record<ListKind, MessageKey[]> = {
  rules: ['rules.help1', 'rules.help2', 'rules.help3'],
  subagents: ['subagents.help1', 'subagents.help2'],
  commands: ['commands.help1', 'commands.help2'],
  plugins: ['plugins.help1', 'plugins.help2'],
};

export const listHelp = (kind: ListKind) => ({ title: t(`${kind}.helpTitle`), paragraphs: HELP[kind].map((k) => t(k)) });

interface Item {
  scope: ExtensionScope;
  path: string;
  /** Opens in the viewer (markdown files only). */
  viewable: boolean;
  aside?: string | null;
  /** A rules file: exists / size. */
  rules?: RulesFileInfo;
}

interface Row {
  key: string;
  name: string;
  description: string;
  scope: ExtensionScope;
  cells: Partial<Record<AgentKind, Item[]>>;
}

function rowsOf(kind: ListKind, all: HarnessExtensions[]): Row[] {
  const map = new Map<string, Row>();
  const add = (key: string, name: string, description: string, scope: ExtensionScope, agent: AgentKind, item: Item) => {
    const row = map.get(key) ?? { key, name, description, scope, cells: {} };
    if (!row.description && description) row.description = description;
    (row.cells[agent] ??= []).push(item);
    map.set(key, row);
  };
  for (const ext of all) {
    if (kind === 'rules') for (const r of ext.rules) add(`${r.scope}:${r.label}`, r.label, '', r.scope, ext.kind, { scope: r.scope, path: r.path, viewable: false, rules: r });
    if (kind === 'subagents') for (const a of ext.agents) add(a.name, a.name, a.description, a.scope, ext.kind, { scope: a.scope, path: a.path, viewable: a.path.endsWith('.md'), aside: a.model });
    if (kind === 'commands') for (const c of ext.commands) add(c.name, `/${c.name}`, c.description, c.scope, ext.kind, { scope: c.scope, path: c.path, viewable: true });
    if (kind === 'plugins')
      for (const p of ext.plugins)
        add(p.name, p.name, '', p.scope, ext.kind, { scope: p.scope, path: p.path, viewable: false, aside: [p.version, p.enabled ? t('ext.enabled') : t('ext.disabled')].filter(Boolean).join(' · ') });
  }
  const order: ExtensionScope[] = ['user', 'project', 'plugin'];
  return [...map.values()].sort((a, b) => (kind === 'rules' ? order.indexOf(a.scope) - order.indexOf(b.scope) || a.name.localeCompare(b.name) : a.name.localeCompare(b.name)));
}

export function HarnessList({ kind }: { kind: ListKind }) {
  const [project, setProject] = useProjectParam();
  const p = project || undefined;
  const { data, loading, error, reload } = useApi(['harness-ext-all', project], () => api.extensionsAll(p));
  const agents = useAgents()?.agents;
  const [query, setQuery] = useState('');
  const [view, setView] = useState<{ agent: AgentKind; path: string; title: string } | null>(null);
  const [edit, setEdit] = useState<{ agent: AgentKind; file: RulesFileInfo } | null>(null);
  const label = (k: AgentKind) => agents?.find((a) => a.kind === k)?.label ?? k;
  const heads = (data?.agents ?? []).map((e) => ({ kind: e.kind, label: label(e.kind) }));
  const all = data ? rowsOf(kind, data.agents) : [];
  const rows = all.filter((r) => matches(query, r.name, r.description, ...Object.values(r.cells).flatMap((c) => c!.map((i) => i.path))));

  return (
    <ExtPage
      title={t(`${kind}.title`)}
      intro={t(`${kind}.intro`)}
      help={listHelp(kind)}
      project={project}
      onProject={setProject}
      query={query}
      onQuery={setQuery}
      searchPlaceholder={t('ext.searchHere')}
      error={error}
      loading={!data && loading}
    >
      {data ? (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[600px] text-sm">
            <thead>
              <tr className="border-b border-line/60 text-left text-xs text-muted">
                <th className="px-4 py-2 font-normal sm:px-5">{t(`${kind}.item`)}</th>
                <AgentHeads agents={heads} />
              </tr>
            </thead>
            <tbody className="divide-y divide-line/50">
              {rows.map((r) => (
                <tr key={r.key} className="align-top">
                  <td className="max-w-sm px-4 py-2.5 sm:px-5">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className="font-medium break-all">{r.name}</span>
                      {kind === 'rules' ? <span className="text-xs text-muted">{r.scope === 'project' ? t('rules.project') : t('rules.global')}</span> : null}
                    </div>
                    {r.description ? <p className="mt-0.5 line-clamp-2 text-xs text-muted">{r.description}</p> : null}
                  </td>
                  {heads.map((h) => (
                    <td key={h.kind} className="px-2 py-2.5 text-center">
                      <div className="flex flex-col items-center gap-1">
                        {(r.cells[h.kind] ?? []).map((it) =>
                          it.rules ? (
                            <button
                              key={it.path}
                              type="button"
                              title={it.path}
                              onClick={() => setEdit({ agent: h.kind, file: it.rules! })}
                              className="inline-flex items-center gap-1 rounded-md border border-line px-1.5 py-0.5 text-[11px] text-muted hover:bg-sunken hover:text-fg"
                            >
                              {it.rules.exists ? <Pencil className="size-3" /> : <Plus className="size-3" />}
                              {it.rules.exists ? fmtSize(it.rules.size) : t('ext.create')}
                            </button>
                          ) : (
                            <button
                              key={it.path}
                              type="button"
                              title={it.path}
                              disabled={!it.viewable}
                              onClick={() => setView({ agent: h.kind, path: it.path, title: r.name })}
                              className="inline-flex flex-col items-center gap-0.5 disabled:cursor-default"
                            >
                              <ScopeBadge scope={it.scope} />
                              {it.aside ? <span className="font-mono text-[10px] text-muted">{it.aside}</span> : null}
                            </button>
                          ),
                        )}
                        {!r.cells[h.kind]?.length ? <span className="text-muted/50">—</span> : null}
                      </div>
                    </td>
                  ))}
                </tr>
              ))}
              {!all.length ? <EmptyRow cols={heads.length + 1} text={t(`${kind}.empty`)} /> : !rows.length ? <EmptyRow cols={heads.length + 1} text={t('ext.noMatch')} /> : null}
            </tbody>
          </table>
        </div>
      ) : null}
      <ViewFile target={view} project={p} onClose={() => setView(null)} />
      <RulesEditor target={edit} project={p} onClose={() => setEdit(null)} onSaved={reload} />
    </ExtPage>
  );
}

const fmtSize = (n: number) => (n < 1024 ? `${n} B` : `${(n / 1024).toFixed(1)} KB`);

function ViewFile({ target, project, onClose }: { target: { agent: AgentKind; path: string; title: string } | null; project?: string; onClose: () => void }) {
  const { data, loading, error } = useApi(['harness-ext-file', target?.agent, target?.path, project], () => api.extensionFile(target!.agent, target!.path, project), { enabled: target !== null });
  const shown = data && data.path === target?.path ? data : undefined;
  return <FileViewer open={target !== null} title={target?.title ?? ''} path={target?.path} content={shown?.content} loading={loading && !shown} error={error} note={t('ext.readOnly')} onClose={onClose} />;
}

/** A rules file in an editor; saves only files the server lists. */
function RulesEditor({ target, project, onClose, onSaved }: { target: { agent: AgentKind; file: RulesFileInfo } | null; project?: string; onClose: () => void; onSaved: () => Promise<void> }) {
  const { data, loading, error } = useApi(['harness-rules-file', target?.agent, target?.file.path, project], () => api.rulesFile(target!.agent, target!.file.path, project), {
    enabled: target !== null,
    staleTime: 0,
  });
  const [text, setText] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const loaded = data && data.path === target?.file.path ? data : undefined;
  const value = text ?? loaded?.content ?? '';
  const close = () => {
    setText(null);
    setSaveError(null);
    onClose();
  };
  const save = async () => {
    if (!target) return;
    setSaveError(null);
    setBusy(true);
    try {
      await api.saveRulesFile(target.agent, target.file.path, value, project);
      await onSaved();
      close();
    } catch (e) {
      setSaveError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Sheet
      open={target !== null}
      onClose={close}
      title={target ? target.file.label : ''}
      wide
      footer={
        <>
          <Button size="sm" variant="primary" loading={busy} disabled={!loaded || (text === null && loaded.exists)} onClick={() => void save()}>
            {t('common.save')}
          </Button>
          <Button size="sm" variant="ghost" onClick={close}>
            {t('common.cancel')}
          </Button>
        </>
      }
    >
      <div className="space-y-2">
        <div className="font-mono text-[11px] break-all text-muted">{target?.file.path}</div>
        <p className="text-xs text-muted">{target?.file.scope === 'project' ? t('rules.editProjectHint') : t('rules.editHint')}</p>
        {error ? <ErrorLine error={error} /> : null}
        {saveError ? <ErrorLine error={saveError} /> : null}
        {loading && !loaded ? (
          <div className="flex justify-center py-6">
            <Spinner />
          </div>
        ) : (
          <textarea
            value={value}
            onChange={(e) => setText(e.target.value)}
            spellCheck={false}
            rows={Math.min(28, Math.max(10, value.split('\n').length + 1))}
            className="w-full resize-y border border-line bg-bg px-3 py-2 font-mono text-[13px] outline-none focus:border-accent"
            aria-label={target?.file.path}
          />
        )}
      </div>
    </Sheet>
  );
}

export const HarnessRules = () => <HarnessList key="rules" kind="rules" />;
export const HarnessSubagents = () => <HarnessList key="subagents" kind="subagents" />;
export const HarnessCommands = () => <HarnessList key="commands" kind="commands" />;
export const HarnessPlugins = () => <HarnessList key="plugins" kind="plugins" />;
