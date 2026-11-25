import { useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import type { AgentKind, McpAgentInfo, McpResponse, McpServerInfo, McpTransport } from '@hubness/shared';
import { api } from '../lib/api';
import { useApi } from '../lib/query';
import { AgentIcon } from '../components/AgentIcon';
import { ProjectSelect, ScopeBadge } from '../components/HarnessExtensions';
import { Select } from '../components/Select';
import { Button, Checkbox, ErrorLine, Field, Spinner, inputCls } from '../components/ui';
import { t } from '../i18n';

/**
 * Harness → MCP: servers × agents, read from each agent's own config file. Toggling a cell
 * connects (copies the definition from another agent) or disconnects the server in that
 * agent's user config, so the agent sees it outside the hub too. Project servers and the
 * hub's built-in server are shown read-only.
 */
export function HarnessMcp() {
  const [project, setProject] = useState('');
  const { data, setData, loading, error } = useApi(['harness-mcp', project], () => api.mcp(project || undefined));
  const [adding, setAdding] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [actError, setActError] = useState<string | null>(null);
  const act = async (key: string, fn: () => Promise<McpResponse>) => {
    setActError(null);
    setBusy(key);
    try {
      setData(await fn());
    } catch (e) {
      setActError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };
  const p = project || undefined;

  return (
    <div className="space-y-3">
      <section className="overflow-hidden rounded-2xl border border-line bg-surface">
        <header className="flex flex-wrap items-center gap-2 px-4 py-4 sm:px-5">
          <div className="min-w-0 flex-1">
            <h2 className="text-[17px] font-semibold">{t('mcp.title')}</h2>
            <p className="text-xs text-muted">{t('mcp.hint')}</p>
          </div>
          <ProjectSelect value={project} onChange={setProject} />
          <Button size="sm" variant="primary" onClick={() => setAdding((v) => !v)}>
            <Plus className="size-3.5" /> {t('mcp.add')}
          </Button>
        </header>
        {adding && data ? <AddForm agents={data.agents} project={p} onDone={(r) => (setData(r), setAdding(false))} onCancel={() => setAdding(false)} /> : null}
        <div className="border-t border-line/60">
          {error ? (
            <div className="p-4">
              <ErrorLine error={error} />
            </div>
          ) : !data && loading ? (
            <div className="flex justify-center py-8">
              <Spinner />
            </div>
          ) : data ? (
            <Matrix data={data} busy={busy} act={act} project={p} />
          ) : null}
        </div>
        {actError ? (
          <div className="border-t border-line/60 p-3">
            <ErrorLine error={actError} />
          </div>
        ) : null}
      </section>
      {data ? <ConfigFiles agents={data.agents} /> : null}
    </div>
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

const canHave = (a: McpAgentInfo, s: McpServerInfo) => a.supported && !a.error && a.transports.includes(s.transport);

function Matrix({ data, busy, act, project }: { data: McpResponse; busy: string | null; act: (key: string, fn: () => Promise<McpResponse>) => Promise<void>; project?: string }) {
  const empty = !data.servers.some((s) => !s.builtIn);
  return (
    <div className="overflow-x-auto">
      <table className="w-full min-w-[560px] text-sm">
        <thead>
          <tr className="border-b border-line/60 text-left text-xs text-muted">
            <th className="px-4 py-2 font-normal sm:px-5">{t('mcp.server')}</th>
            {data.agents.map((a) => (
              <th key={a.kind} className="px-2 py-2 text-center font-normal">
                <span className="inline-flex flex-col items-center gap-1">
                  <AgentIcon kind={a.kind} className="size-4" />
                  <span className="whitespace-nowrap">{a.label}</span>
                </span>
              </th>
            ))}
            <th className="w-10" />
          </tr>
        </thead>
        <tbody className="divide-y divide-line/50">
          {data.servers.map((s) => (
            <tr key={s.name} className="align-top">
              <td className="px-4 py-2.5 sm:px-5">
                <ServerCell s={s} />
              </td>
              {data.agents.map((a) => (
                <td key={a.kind} className="px-2 py-2.5 text-center">
                  <Cell s={s} a={a} busy={busy === `${s.name}:${a.kind}`} act={act} project={project} />
                </td>
              ))}
              <td className="py-2.5 pr-3 text-right">
                {!s.builtIn && s.entries.some((e) => e.scope === 'user') ? <DeleteButton s={s} act={act} project={project} /> : null}
              </td>
            </tr>
          ))}
          {empty ? (
            <tr>
              <td colSpan={data.agents.length + 2} className="px-4 py-4 text-sm text-muted sm:px-5">
                {t('mcp.empty')}
              </td>
            </tr>
          ) : null}
        </tbody>
      </table>
    </div>
  );
}

function ServerCell({ s }: { s: McpServerInfo }) {
  const target = s.transport === 'stdio' ? [s.command, ...s.args].filter(Boolean).join(' ') : s.url;
  const keys = s.transport === 'stdio' ? s.env : s.headers;
  return (
    <div className="min-w-0">
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="font-medium">{s.builtIn ? t('mcp.builtIn') : s.name}</span>
        <span className="rounded border border-line px-1 font-mono text-[10px] text-muted uppercase">{s.transport}</span>
      </div>
      {s.builtIn ? (
        <div className="text-xs text-muted">{t('mcp.builtInHint')}</div>
      ) : target ? (
        <div className="max-w-xs truncate font-mono text-[11px] text-muted" title={target}>
          {target}
        </div>
      ) : null}
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

function Cell({ s, a, busy, act, project }: { s: McpServerInfo; a: McpAgentInfo; busy: boolean; act: (key: string, fn: () => Promise<McpResponse>) => Promise<void>; project?: string }) {
  const state = s.agents[a.kind] ?? null;
  if (state === 'builtin') return <ScopeBadge scope="builtin" />;
  const entry = s.entries.find((e) => e.agent === a.kind);
  if (state === 'project')
    return (
      <span title={entry?.source}>
        <ScopeBadge scope="project" />
      </span>
    );
  const on = state === 'user';
  const disabled = busy || (!on && !canHave(a, s)) || Boolean(on && a.error);
  const title = !on && !a.transports.includes(s.transport) ? t('mcp.noTransport', { transport: s.transport.toUpperCase() }) : a.error ?? entry?.source ?? (on ? t('mcp.disconnect') : t('mcp.connect'));
  return (
    <span className="inline-flex flex-col items-center gap-0.5">
      <button
        type="button"
        role="switch"
        aria-checked={on}
        aria-label={`${s.name} · ${a.label}`}
        title={title}
        disabled={disabled}
        onClick={() =>
          void act(`${s.name}:${a.kind}`, () => (on ? api.mcpDisconnect(s.name, a.kind, project) : api.mcpConnect(s.name, a.kind, project)))
        }
        className={`flex h-5 w-9 shrink-0 items-center rounded-full border transition disabled:opacity-40 ${on ? 'border-accent bg-accent' : 'border-line bg-sunken'}`}
      >
        <span className={`size-3.5 rounded-full bg-surface shadow transition ${on ? 'translate-x-4.5' : 'translate-x-0.5'}`} />
      </button>
      {on && entry && !entry.enabled ? <span className="text-[10px] text-warn">{t('mcp.turnedOff')}</span> : null}
    </span>
  );
}

function DeleteButton({ s, act, project }: { s: McpServerInfo; act: (key: string, fn: () => Promise<McpResponse>) => Promise<void>; project?: string }) {
  const [armed, setArmed] = useState(false);
  return armed ? (
    <span className="inline-flex flex-col items-end gap-1">
      <button type="button" className="rounded-md border border-danger/40 px-2 py-0.5 text-xs whitespace-nowrap text-danger hover:bg-danger/10" onClick={() => void act(`${s.name}:delete`, () => api.mcpDelete(s.name, project))}>
        {t('mcp.deleteConfirm')}
      </button>
      <button type="button" className="text-xs text-muted hover:text-fg" onClick={() => setArmed(false)}>
        {t('common.cancel')}
      </button>
    </span>
  ) : (
    <button type="button" title={t('mcp.delete')} aria-label={t('mcp.delete')} className="rounded-md p-1 text-muted hover:bg-sunken hover:text-danger" onClick={() => setArmed(true)}>
      <Trash2 className="size-4" />
    </button>
  );
}

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

function AddForm({ agents, project, onDone, onCancel }: { agents: McpAgentInfo[]; project?: string; onDone: (r: McpResponse) => void; onCancel: () => void }) {
  const [name, setName] = useState('');
  const [transport, setTransport] = useState<McpTransport>('stdio');
  const [command, setCommand] = useState('');
  const [args, setArgs] = useState('');
  const [env, setEnv] = useState('');
  const [url, setUrl] = useState('');
  const [headers, setHeaders] = useState('');
  const [picked, setPicked] = useState<AgentKind[]>(agents.filter((a) => a.supported && !a.error).map((a) => a.kind));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const usable = (a: McpAgentInfo) => a.supported && !a.error && a.transports.includes(transport);
  const submit = async () => {
    setError(null);
    const pairs = transport === 'stdio' ? parsePairs(env, '=') : parsePairs(headers, ':');
    if ('bad' in pairs) return setError(t('mcp.badLine', { line: pairs.bad }));
    setBusy(true);
    try {
      const kinds = picked.filter((k) => usable(agents.find((a) => a.kind === k)!));
      const r = await api.mcpAdd(
        transport === 'stdio'
          ? { name: name.trim(), transport, command: command.trim(), args: args.split('\n').map((x) => x.trim()).filter(Boolean), env: pairs.ok, agents: kinds }
          : { name: name.trim(), transport, url: url.trim(), headers: pairs.ok, agents: kinds },
        project,
      );
      onDone(r);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const area = `${inputCls} min-h-16 font-mono text-[13px]`;
  return (
    <form
      className="space-y-3 border-t border-line/60 bg-bg/40 px-4 py-4 sm:px-5"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={t('mcp.name')} hint={t('mcp.nameHint')}>
          <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} placeholder="github" autoFocus />
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
              <textarea className={area} value={args} onChange={(e) => setArgs(e.target.value)} placeholder={'-y\n@modelcontextprotocol/server-github'} spellCheck={false} />
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
      <div>
        <div className="mb-1.5 text-xs text-muted">{t('mcp.agents')}</div>
        <div className="flex flex-wrap gap-x-4 gap-y-2">
          {agents.map((a) => (
            <Checkbox
              key={a.kind}
              checked={picked.includes(a.kind) && usable(a)}
              disabled={!usable(a)}
              onChange={(v) => setPicked((cur) => (v ? [...cur, a.kind] : cur.filter((k) => k !== a.kind)))}
            >
              <span className="inline-flex items-center gap-1.5 text-sm">
                <AgentIcon kind={a.kind} className="size-4" /> {a.label}
              </span>
            </Checkbox>
          ))}
        </div>
      </div>
      <p className="text-xs text-muted">{t('mcp.secretsHint')}</p>
      {error ? <ErrorLine error={error} /> : null}
      <div className="flex gap-2">
        <Button type="submit" size="sm" variant="primary" loading={busy}>
          {t('mcp.save')}
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={onCancel}>
          {t('common.cancel')}
        </Button>
      </div>
    </form>
  );
}
