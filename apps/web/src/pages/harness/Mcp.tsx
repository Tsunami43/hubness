import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { ArrowLeft, ExternalLink, Info, Library, Pencil, Plus, Trash2 } from 'lucide-react';
import type { AgentKind, McpAgentInfo, McpCatalogEntry, McpCatalogVar, McpInstallOption, McpResponse, McpServerInfo, McpTransport } from '@hubness/shared';
import { api } from '../../lib/api';
import { useApi } from '../../lib/query';
import { AgentIcon } from '../../components/AgentIcon';
import { ScopeBadge } from '../../components/HarnessExtensions';
import { Select } from '../../components/Select';
import { confirmDialog } from '../../components/Confirm';
import { Button, Checkbox, ErrorLine, Field, Sheet, Spinner, inputCls } from '../../components/ui';
import { AgentHeads, EmptyRow, ExtPage, RowMenu, Toggle, errText, matches, useProjectParam } from '../../components/harness/kit';
import { t } from '../../i18n';

/**
 * Harness → Extensions → MCP servers: servers × agents, read from each agent's own config
 * file. A cell connects (copies the definition from another agent) or disconnects the
 * server in that agent's user config. New servers come from the catalog (the MCP Registry
 * through the hub, with a built-in popular list) or are added by hand. Project servers and
 * the hub's built-in server are read-only.
 */

type Act = (key: string, fn: () => Promise<McpResponse>) => Promise<void>;

export const mcpHelp = () => ({
  title: t('mcp.helpTitle'),
  paragraphs: [t('mcp.help1'), t('mcp.help2'), t('mcp.help3'), t('mcp.help4'), t('mcp.help5')],
});

export function HarnessMcp() {
  const [project, setProject] = useProjectParam();
  const p = project || undefined;
  const { data, setData, loading, error } = useApi(['harness-mcp', project], () => api.mcp(p));
  const [query, setQuery] = useState('');
  const [catalog, setCatalog] = useState(false);
  const [form, setForm] = useState<FormInit | null>(null);
  const [details, setDetails] = useState<McpServerInfo | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [actError, setActError] = useState<string | null>(null);
  const act: Act = async (key, fn) => {
    setActError(null);
    setBusy(key);
    try {
      setData(await fn());
    } catch (e) {
      setActError(errText(e));
    } finally {
      setBusy(null);
    }
  };
  const remove = async (s: McpServerInfo) => {
    const ok = await confirmDialog({ title: t('mcp.delete'), message: t('mcp.deleteAsk', { name: s.name }), confirmLabel: t('mcp.deleteConfirm'), danger: true });
    if (ok) await act(`${s.name}:delete`, () => api.mcpDelete(s.name, p));
  };
  const rows = (data?.servers ?? []).filter((s) => matches(query, s.name, s.command, s.url, ...s.args));

  return (
    <ExtPage
      title={t('mcp.title')}
      intro={t('mcp.intro')}
      help={mcpHelp()}
      project={project}
      onProject={setProject}
      query={query}
      onQuery={setQuery}
      searchPlaceholder={t('mcp.searchInstalled')}
      error={error}
      loading={!data && loading}
      actions={
        <>
          <Button size="sm" variant="primary" onClick={() => setCatalog(true)}>
            <Library className="size-3.5" /> {t('mcp.catalog')}
          </Button>
          <Button size="sm" onClick={() => setForm({})}>
            <Plus className="size-3.5" /> {t('mcp.addManual')}
          </Button>
        </>
      }
      footer={data ? <ConfigFiles agents={data.agents} /> : null}
    >
      {data ? (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[600px] text-sm">
            <thead>
              <tr className="border-b border-line/60 text-left text-xs text-muted">
                <th className="px-4 py-2 font-normal sm:px-5">{t('mcp.server')}</th>
                <AgentHeads agents={data.agents} />
                <th className="w-10" />
              </tr>
            </thead>
            <tbody className="divide-y divide-line/50">
              {rows.map((s) => (
                <tr key={s.name} className="align-top">
                  <td className="px-4 py-2.5 sm:px-5">
                    <ServerCell s={s} onOpen={() => setDetails(s)} />
                  </td>
                  {data.agents.map((a) => (
                    <td key={a.kind} className="px-2 py-2.5 text-center">
                      <Cell s={s} a={a} busy={busy === `${s.name}:${a.kind}`} act={act} project={p} />
                    </td>
                  ))}
                  <td className="py-2 pr-3 text-right">
                    {s.builtIn ? null : (
                      <RowMenu
                        label={t('common.actions')}
                        items={[
                          { label: t('mcp.details'), icon: <Info className="size-3.5" />, onClick: () => setDetails(s) },
                          { label: t('mcp.edit'), icon: <Pencil className="size-3.5" />, onClick: () => setForm(editInit(s)) },
                          {
                            label: t('mcp.delete'),
                            icon: <Trash2 className="size-3.5" />,
                            danger: true,
                            disabled: !s.entries.some((e) => e.scope === 'user'),
                            onClick: () => void remove(s),
                          },
                        ]}
                      />
                    )}
                  </td>
                </tr>
              ))}
              {!data.servers.some((s) => !s.builtIn) ? <EmptyRow cols={data.agents.length + 2} text={t('mcp.empty')} /> : !rows.length ? <EmptyRow cols={data.agents.length + 2} text={t('ext.noMatch')} /> : null}
            </tbody>
          </table>
        </div>
      ) : null}
      {actError ? (
        <div className="border-t border-line/60 p-3">
          <ErrorLine error={actError} />
        </div>
      ) : null}
      {data ? (
        <>
          <CatalogSheet
            open={catalog}
            agents={data.agents}
            existing={data.servers.map((s) => s.name)}
            project={p}
            onClose={() => setCatalog(false)}
            onDone={(r) => {
              setData(r);
              setCatalog(false);
            }}
          />
          <Sheet open={form !== null} onClose={() => setForm(null)} title={form?.name ? t('mcp.editTitle', { name: form.name }) : t('mcp.addManual')}>
            {form ? (
              <ManualForm
                init={form}
                agents={data.agents}
                project={p}
                onDone={(r) => {
                  setData(r);
                  setForm(null);
                }}
                onCancel={() => setForm(null)}
              />
            ) : null}
          </Sheet>
          <DetailsSheet s={details} agents={data.agents} onClose={() => setDetails(null)} />
        </>
      ) : null}
    </ExtPage>
  );
}

function ConfigFiles({ agents }: { agents: McpAgentInfo[] }) {
  return (
    <section className="rounded-2xl border border-line bg-surface px-4 py-3 sm:px-5">
      <h3 className="mb-2 text-sm font-semibold">{t('mcp.files')}</h3>
      <ul className="space-y-1.5">
        {agents.map((a) => (
          <li key={a.kind} className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-sm">
            <AgentIcon kind={a.kind} className="size-4" />
            <span className="w-28 shrink-0">{a.label}</span>
            <code className="min-w-0 flex-1 truncate font-mono text-xs text-muted" title={a.configPath ?? ''}>
              {a.configPath ?? t('mcp.unsupported')}
            </code>
            {a.error ? <span className="w-full text-xs text-danger">{a.error}</span> : null}
          </li>
        ))}
      </ul>
    </section>
  );
}

const canHave = (a: McpAgentInfo, transport: McpTransport) => a.supported && !a.error && a.transports.includes(transport);
const target = (s: Pick<McpServerInfo, 'transport' | 'command' | 'args' | 'url'>) => (s.transport === 'stdio' ? [s.command, ...s.args].filter(Boolean).join(' ') : (s.url ?? ''));

function ServerCell({ s, onOpen }: { s: McpServerInfo; onOpen: () => void }) {
  const keys = s.transport === 'stdio' ? s.env : s.headers;
  return (
    <div className="min-w-0">
      <div className="flex flex-wrap items-center gap-1.5">
        {s.builtIn ? (
          <span className="font-medium">{t('mcp.builtIn')}</span>
        ) : (
          <button type="button" onClick={onOpen} className="font-medium hover:text-accent hover:underline">
            {s.name}
          </button>
        )}
        <span className="rounded border border-line px-1 font-mono text-[10px] text-muted uppercase">{s.transport}</span>
      </div>
      {s.builtIn ? (
        <div className="text-xs text-muted">{t('mcp.builtInHint')}</div>
      ) : (
        <div className="max-w-xs truncate font-mono text-[11px] text-muted" title={target(s)}>
          {target(s)}
        </div>
      )}
      {!s.builtIn && keys.length ? (
        <div className="mt-1 flex flex-wrap gap-1">
          {keys.map((k) => (
            <span key={k} className="rounded bg-sunken px-1 font-mono text-[10px] text-muted" title={t('mcp.valueHidden')}>
              {k}
            </span>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function Cell({ s, a, busy, act, project }: { s: McpServerInfo; a: McpAgentInfo; busy: boolean; act: Act; project?: string }) {
  const state = s.agents[a.kind] ?? null;
  if (state === 'builtin') return <ScopeBadge scope="builtin" />;
  const entry = s.entries.find((e) => e.agent === a.kind);
  if (state === 'project')
    return (
      <span title={t('ext.projectReadOnly', { path: entry?.source ?? '' })}>
        <ScopeBadge scope="project" />
      </span>
    );
  const on = state === 'user';
  const disabled = busy || (!on && !canHave(a, s.transport)) || Boolean(on && a.error);
  const title = !on && !a.transports.includes(s.transport) ? t('mcp.noTransport', { transport: s.transport.toUpperCase() }) : (a.error ?? (on ? t('mcp.disconnect') : t('mcp.connect')));
  return (
    <span className="inline-flex flex-col items-center gap-0.5">
      <Toggle
        on={on}
        disabled={disabled}
        title={title}
        label={`${s.name} · ${a.label}`}
        onClick={() => void act(`${s.name}:${a.kind}`, () => (on ? api.mcpDisconnect(s.name, a.kind, project) : api.mcpConnect(s.name, a.kind, project)))}
      />
      {on && entry && !entry.enabled ? <span className="text-[10px] text-warn">{t('mcp.turnedOff')}</span> : null}
    </span>
  );
}

function DetailsSheet({ s, agents, onClose }: { s: McpServerInfo | null; agents: McpAgentInfo[]; onClose: () => void }) {
  const label = (k: AgentKind) => agents.find((a) => a.kind === k)?.label ?? k;
  const keys = s ? (s.transport === 'stdio' ? s.env : s.headers) : [];
  return (
    <Sheet open={s !== null} onClose={onClose} title={s?.name ?? ''}>
      {s ? (
        <dl className="space-y-3 text-sm">
          <Row label={t('mcp.transport')}>{s.transport === 'stdio' ? t('mcp.stdio') : s.transport.toUpperCase()}</Row>
          <Row label={s.transport === 'stdio' ? t('mcp.command') : 'URL'}>
            <code className="font-mono text-[12.5px] break-all">{target(s) || '—'}</code>
          </Row>
          <Row label={s.transport === 'stdio' ? t('mcp.env') : t('mcp.headers')}>
            {keys.length ? (
              <span className="flex flex-wrap gap-1">
                {keys.map((k) => (
                  <code key={k} className="rounded bg-sunken px-1 font-mono text-[11px]">
                    {k}
                  </code>
                ))}
              </span>
            ) : (
              '—'
            )}
            {keys.length ? <div className="mt-1 text-xs text-muted">{t('mcp.valuesHidden')}</div> : null}
          </Row>
          <Row label={t('mcp.sources')}>
            <ul className="space-y-1.5">
              {s.entries.map((e) => (
                <li key={`${e.agent}:${e.source}`} className="flex flex-wrap items-center gap-1.5">
                  <AgentIcon kind={e.agent} className="size-4" />
                  <span>{label(e.agent)}</span>
                  <ScopeBadge scope={e.scope} />
                  {!e.enabled ? <span className="text-xs text-warn">{t('mcp.turnedOff')}</span> : null}
                  <code className="w-full truncate font-mono text-[11px] text-muted" title={e.source}>
                    {e.source}
                  </code>
                </li>
              ))}
            </ul>
          </Row>
        </dl>
      ) : null}
    </Sheet>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div>
      <dt className="mb-0.5 text-xs text-muted">{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

// ---------- adding by hand / editing ----------

interface FormInit {
  name?: string;
  transport?: McpTransport;
  command?: string;
  args?: string[];
  url?: string;
  /** Names of env vars / headers whose values have to be entered again. */
  keys?: string[];
  agents?: AgentKind[];
}

const editInit = (s: McpServerInfo): FormInit => ({
  name: s.name,
  transport: s.transport,
  command: s.command ?? '',
  args: s.args,
  url: s.url ?? '',
  keys: s.transport === 'stdio' ? s.env : s.headers,
  agents: s.entries.filter((e) => e.scope === 'user').map((e) => e.agent),
});

/** `KEY=value` / `Name: value` lines → a map, or the first line that isn't one. */
function parsePairs(text: string, sep: '=' | ':'): { ok: Record<string, string> } | { bad: string } {
  const out: Record<string, string> = {};
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line) continue;
    const i = line.indexOf(sep);
    if (i <= 0) return { bad: line };
    out[line.slice(0, i).trim()] = line.slice(i + 1).trim();
  }
  return { ok: out };
}

function AgentChecks({ agents, picked, setPicked, transport }: { agents: McpAgentInfo[]; picked: AgentKind[]; setPicked: (f: (cur: AgentKind[]) => AgentKind[]) => void; transport: McpTransport }) {
  return (
    <div>
      <div className="mb-1.5 text-xs text-muted">{t('mcp.agents')}</div>
      <div className="flex flex-wrap gap-x-4 gap-y-2">
        {agents.map((a) => {
          const ok = canHave(a, transport);
          const why = !a.supported || a.error ? (a.error ?? t('mcp.unsupported')) : !a.transports.includes(transport) ? t('mcp.noTransport', { transport: transport.toUpperCase() }) : undefined;
          return (
            <span key={a.kind} title={why}>
              <Checkbox checked={picked.includes(a.kind) && ok} disabled={!ok} onChange={(v) => setPicked((cur) => (v ? [...cur, a.kind] : cur.filter((k) => k !== a.kind)))}>
                <span className="inline-flex items-center gap-1.5 text-sm">
                  <AgentIcon kind={a.kind} className="size-4" /> {a.label}
                </span>
                {why ? <span className="block text-[11px] text-muted">{why}</span> : null}
              </Checkbox>
            </span>
          );
        })}
      </div>
    </div>
  );
}

function ManualForm({ init, agents, project, onDone, onCancel }: { init: FormInit; agents: McpAgentInfo[]; project?: string; onDone: (r: McpResponse) => void; onCancel: () => void }) {
  const editing = Boolean(init.name);
  const [name, setName] = useState(init.name ?? '');
  const [transport, setTransport] = useState<McpTransport>(init.transport ?? 'stdio');
  const [command, setCommand] = useState(init.command ?? '');
  const [args, setArgs] = useState((init.args ?? []).join('\n'));
  const [env, setEnv] = useState(init.transport !== 'stdio' ? '' : (init.keys ?? []).map((k) => `${k}=`).join('\n'));
  const [url, setUrl] = useState(init.url ?? '');
  const [headers, setHeaders] = useState(init.transport === 'stdio' ? '' : (init.keys ?? []).map((k) => `${k}: `).join('\n'));
  const [picked, setPicked] = useState<AgentKind[]>(init.agents ?? agents.filter((a) => a.supported && !a.error).map((a) => a.kind));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async () => {
    setError(null);
    const pairs = transport === 'stdio' ? parsePairs(env, '=') : parsePairs(headers, ':');
    if ('bad' in pairs) return setError(t('mcp.badLine', { line: pairs.bad }));
    setBusy(true);
    try {
      const kinds = picked.filter((k) => canHave(agents.find((a) => a.kind === k)!, transport));
      onDone(
        await api.mcpAdd(
          transport === 'stdio'
            ? { name: name.trim(), transport, command: command.trim(), args: args.split('\n').map((x) => x.trim()).filter(Boolean), env: pairs.ok, agents: kinds }
            : { name: name.trim(), transport, url: url.trim(), headers: pairs.ok, agents: kinds },
          project,
        ),
      );
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  const area = `${inputCls} min-h-16 font-mono text-[13px]`;
  return (
    <form
      className="space-y-3"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      {editing ? <p className="text-xs text-warn">{t('mcp.editHint')}</p> : null}
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={t('mcp.name')} hint={t('mcp.nameHint')}>
          <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} placeholder="github" readOnly={editing} autoFocus={!editing} />
        </Field>
        <div>
          <div className="mb-1 text-xs text-muted">{t('mcp.transport')}</div>
          <Select<McpTransport>
            value={transport}
            onChange={setTransport}
            label={t('mcp.transport')}
            className="w-full"
            options={[
              { value: 'stdio', label: t('mcp.stdio') },
              { value: 'http', label: 'HTTP' },
              { value: 'sse', label: 'SSE' },
            ]}
          />
        </div>
      </div>
      {transport === 'stdio' ? (
        <>
          <Field label={t('mcp.command')}>
            <input className={`${inputCls} font-mono`} value={command} onChange={(e) => setCommand(e.target.value)} placeholder="npx" />
          </Field>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label={t('mcp.args')} hint={t('mcp.argsHint')}>
              <textarea className={area} value={args} onChange={(e) => setArgs(e.target.value)} placeholder={'-y\n@playwright/mcp@latest'} spellCheck={false} />
            </Field>
            <Field label={t('mcp.env')} hint={t('mcp.envHint')}>
              <textarea className={area} value={env} onChange={(e) => setEnv(e.target.value)} placeholder="GITHUB_TOKEN=…" spellCheck={false} />
            </Field>
          </div>
        </>
      ) : (
        <>
          <Field label="URL">
            <input className={`${inputCls} font-mono`} value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://mcp.example.com/mcp" />
          </Field>
          <Field label={t('mcp.headers')} hint={t('mcp.headersHint')}>
            <textarea className={area} value={headers} onChange={(e) => setHeaders(e.target.value)} placeholder="Authorization: Bearer …" spellCheck={false} />
          </Field>
        </>
      )}
      <AgentChecks agents={agents} picked={picked} setPicked={setPicked} transport={transport} />
      <p className="text-xs text-muted">{t('mcp.secretsHint')}</p>
      {error ? <ErrorLine error={error} /> : null}
      <div className="flex gap-2">
        <Button type="submit" size="sm" variant="primary" loading={busy}>
          {editing ? t('common.save') : t('mcp.save')}
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={onCancel}>
          {t('common.cancel')}
        </Button>
      </div>
    </form>
  );
}

// ---------- the catalog ----------

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setV(value), ms);
    return () => clearTimeout(id);
  }, [value, ms]);
  return v;
}

const optionLabel = (o: McpInstallOption) =>
  o.kind === 'remote' ? `${t('mcp.optRemote')} (${o.transport.toUpperCase()}) · ${o.url}` : `${o.kind === 'docker' ? 'Docker' : o.kind === 'pypi' ? 'PyPI' : 'npm'} · ${[o.command, ...o.args].join(' ')}`;

function CatalogSheet({ open, agents, existing, project, onClose, onDone }: { open: boolean; agents: McpAgentInfo[]; existing: string[]; project?: string; onClose: () => void; onDone: (r: McpResponse) => void }) {
  const [q, setQ] = useState('');
  const query = useDebounced(q.trim(), 350);
  const [pick, setPick] = useState<McpCatalogEntry | null>(null);
  const [more, setMore] = useState<{ entries: McpCatalogEntry[]; cursor: string | null; loading: boolean; error: string | null }>({ entries: [], cursor: null, loading: false, error: null });
  const { data, loading, error } = useApi(['mcp-catalog', query], () => api.mcpCatalog(query), { enabled: open, staleTime: 5 * 60_000 });
  useEffect(() => setMore({ entries: [], cursor: data?.nextCursor ?? null, loading: false, error: null }), [data]);
  useEffect(() => {
    if (!open) {
      setPick(null);
      setQ('');
    }
  }, [open]);
  const loadMore = async () => {
    if (!more.cursor) return;
    setMore((m) => ({ ...m, loading: true, error: null }));
    try {
      const r = await api.mcpCatalog(query, more.cursor);
      setMore((m) => ({ entries: [...m.entries, ...r.entries], cursor: r.nextCursor, loading: false, error: null }));
    } catch (e) {
      setMore((m) => ({ ...m, loading: false, error: errText(e) }));
    }
  };
  const entries = [...(data?.entries ?? []), ...more.entries];
  return (
    <Sheet open={open} onClose={onClose} title={pick ? pick.title : t('mcp.catalog')} wide>
      {pick ? (
        <InstallForm entry={pick} agents={agents} existing={existing} project={project} onBack={() => setPick(null)} onDone={onDone} />
      ) : (
        <div className="space-y-3">
          <p className="text-xs text-muted">{t('mcp.catalogHint')}</p>
          <input
            type="search"
            className={inputCls}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder={t('mcp.catalogSearch')}
            aria-label={t('mcp.catalogSearch')}
            autoFocus
          />
          <div className="text-xs font-medium text-muted">{data?.source === 'popular' ? t('mcp.popular') : query ? t('mcp.registryResults') : null}</div>
          {error ? <ErrorLine error={error} /> : null}
          {loading && !data ? (
            <div className="flex justify-center py-6">
              <Spinner />
            </div>
          ) : (
            <ul className="divide-y divide-line/60 border border-line/60">
              {entries.map((e) => (
                <li key={e.id}>
                  <button type="button" onClick={() => setPick(e)} disabled={!e.options.length} className="flex w-full items-start gap-3 px-3 py-2.5 text-left hover:bg-sunken disabled:opacity-60 disabled:hover:bg-transparent">
                    <span className="min-w-0 flex-1">
                      <span className="flex flex-wrap items-center gap-x-2">
                        <span className="font-medium">{e.title}</span>
                        <span className="font-mono text-[11px] text-muted">{e.id}</span>
                        {existing.includes(e.suggestedName) ? <span className="text-[11px] text-ok">{t('mcp.installed')}</span> : null}
                      </span>
                      {e.description ? <span className="mt-0.5 line-clamp-2 block text-xs text-muted">{e.description}</span> : null}
                      <span className="mt-1 flex flex-wrap gap-1">
                        {e.options.length ? (
                          [...new Set(e.options.map((o) => (o.kind === 'remote' ? o.transport.toUpperCase() : o.kind)))].map((k) => (
                            <span key={k} className="rounded border border-line px-1 font-mono text-[10px] text-muted">
                              {k}
                            </span>
                          ))
                        ) : (
                          <span className="text-[11px] text-muted">{t('mcp.noOptions')}</span>
                        )}
                      </span>
                    </span>
                  </button>
                </li>
              ))}
              {!entries.length && data ? <li className="px-3 py-4 text-sm text-muted">{t('mcp.catalogEmpty')}</li> : null}
            </ul>
          )}
          {more.error ? <ErrorLine error={more.error} /> : null}
          {more.cursor && data?.source === 'registry' ? (
            <Button size="sm" variant="ghost" loading={more.loading} onClick={() => void loadMore()}>
              {t('mcp.loadMore')}
            </Button>
          ) : null}
        </div>
      )}
    </Sheet>
  );
}

const PLACEHOLDER = /<[^<>\s]+>|\{[A-Za-z0-9_-]+\}/;

function VarInput({ v, value, onChange }: { v: McpCatalogVar; value: string; onChange: (s: string) => void }) {
  return (
    <Field label={`${v.name}${v.required ? ' *' : ''}`} hint={v.description || undefined}>
      <input
        className={`${inputCls} font-mono`}
        type={v.secret ? 'password' : 'text'}
        autoComplete="off"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={v.secret ? t('mcp.secretValue') : ''}
      />
    </Field>
  );
}

function InstallForm({ entry, agents, existing, project, onBack, onDone }: { entry: McpCatalogEntry; agents: McpAgentInfo[]; existing: string[]; project?: string; onBack: () => void; onDone: (r: McpResponse) => void }) {
  const [idx, setIdx] = useState(0);
  const opt = entry.options[idx]!;
  const [name, setName] = useState(entry.suggestedName);
  const [args, setArgs] = useState(opt.args.join('\n'));
  const [url, setUrl] = useState(opt.url ?? '');
  const vars = opt.kind === 'remote' ? opt.headers : opt.env;
  const [values, setValues] = useState<Record<string, string>>({});
  const [picked, setPicked] = useState<AgentKind[]>(agents.filter((a) => canHave(a, opt.transport)).map((a) => a.kind));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setArgs(opt.args.join('\n'));
    setUrl(opt.url ?? '');
    setValues(Object.fromEntries(vars.map((v) => [v.name, v.value ?? ''])));
  }, [idx]);
  const options = useMemo(() => entry.options.map((o, i) => ({ value: String(i), label: optionLabel(o) })), [entry]);
  const submit = async () => {
    setError(null);
    const argList = args.split('\n').map((x) => x.trim()).filter(Boolean);
    const missing = vars.filter((v) => v.required && !(values[v.name] ?? '').trim()).map((v) => v.name);
    if (missing.length) return setError(t('mcp.fillRequired', { names: missing.join(', ') }));
    const left = [...(opt.kind === 'remote' ? [url] : argList), ...Object.values(values)].find((s) => PLACEHOLDER.test(s));
    if (left) return setError(t('mcp.fillPlaceholders', { text: left.match(PLACEHOLDER)![0] }));
    // optional values left empty are not written
    const filled = Object.fromEntries(Object.entries(values).filter(([, v]) => v.trim() !== ''));
    const kinds = picked.filter((k) => canHave(agents.find((a) => a.kind === k)!, opt.transport));
    setBusy(true);
    try {
      onDone(
        await api.mcpAdd(
          opt.kind === 'remote'
            ? { name: name.trim(), transport: opt.transport, url: url.trim(), headers: filled, agents: kinds }
            : { name: name.trim(), transport: 'stdio', command: opt.command ?? '', args: argList, env: filled, agents: kinds },
          project,
        ),
      );
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="space-y-3"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <button type="button" onClick={onBack} className="inline-flex items-center gap-1 text-xs text-muted hover:text-fg">
        <ArrowLeft className="size-3.5" /> {t('mcp.backToCatalog')}
      </button>
      {entry.description ? <p className="text-sm text-muted">{entry.description}</p> : null}
      {entry.repository || entry.website ? (
        <div className="flex flex-wrap gap-3 text-xs">
          {[entry.repository, entry.website].filter(Boolean).map((u) => (
            <a key={u} href={u!} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 text-accent hover:underline">
              <ExternalLink className="size-3" /> {u!.replace(/^https?:\/\//, '')}
            </a>
          ))}
        </div>
      ) : null}
      <div>
        <div className="mb-1 text-xs text-muted">{t('mcp.installOption')}</div>
        <Select value={String(idx)} onChange={(v) => setIdx(Number(v))} options={options} label={t('mcp.installOption')} className="w-full" />
      </div>
      <Field label={t('mcp.name')} hint={existing.includes(name.trim()) ? t('mcp.nameTaken') : t('mcp.nameHint')}>
        <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      {opt.kind === 'remote' ? (
        <Field label="URL">
          <input className={`${inputCls} font-mono`} value={url} onChange={(e) => setUrl(e.target.value)} />
        </Field>
      ) : (
        <Field label={`${t('mcp.command')}: ${opt.command}`} hint={t('mcp.argsHint')}>
          <textarea className={`${inputCls} min-h-16 font-mono text-[13px]`} rows={Math.min(8, Math.max(2, opt.args.length))} value={args} onChange={(e) => setArgs(e.target.value)} spellCheck={false} />
        </Field>
      )}
      {vars.length ? (
        <div className="space-y-2">
          <div className="text-xs font-medium text-muted">{opt.kind === 'remote' ? t('mcp.headers') : t('mcp.env')}</div>
          {vars.map((v) => (
            <VarInput key={v.name} v={v} value={values[v.name] ?? ''} onChange={(s) => setValues((cur) => ({ ...cur, [v.name]: s }))} />
          ))}
        </div>
      ) : null}
      <AgentChecks agents={agents} picked={picked} setPicked={setPicked} transport={opt.transport} />
      <p className="text-xs text-muted">{t('mcp.secretsHint')}</p>
      {error ? <ErrorLine error={error} /> : null}
      <Button type="submit" size="sm" variant="primary" loading={busy}>
        {t('mcp.install')}
      </Button>
    </form>
  );
}
