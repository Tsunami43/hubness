import { useState } from 'react';
import { Eye, GitBranch, Plus, Trash2 } from 'lucide-react';
import type { AgentKind, SkillAgentInfo, SkillContent, SkillImportPreview, SkillRow, SkillsResponse } from '@hubness/shared';
import { api } from '../../lib/api';
import { useApi } from '../../lib/query';
import { AgentIcon } from '../../components/AgentIcon';
import { ScopeBadge } from '../../components/HarnessExtensions';
import { confirmDialog } from '../../components/Confirm';
import { Button, Checkbox, ErrorLine, Field, Sheet, inputCls } from '../../components/ui';
import { AgentHeads, EmptyRow, ExtPage, FileViewer, RowMenu, Toggle, errText, isConflict, matches, useProjectParam } from '../../components/harness/kit';
import { t } from '../../i18n';

/**
 * Harness → Extensions → Skills: skills × agents, read from each agent's skills folders. A
 * cell copies the skill folder to that agent or moves the agent's copy into the hub trash.
 * New skills are written here or imported from a git repository. Project and plugin skills
 * are read-only.
 */

type Act = (key: string, fn: () => Promise<SkillsResponse>) => Promise<void>;

export const skillsHelp = () => ({
  title: t('skills.helpTitle'),
  paragraphs: [t('skills.help1'), t('skills.help2'), t('skills.help3'), t('skills.help4')],
});

export function HarnessSkills() {
  const [project, setProject] = useProjectParam();
  const p = project || undefined;
  const { data, setData, loading, error } = useApi(['harness-skills', project], () => api.skills(p));
  const [query, setQuery] = useState('');
  const [creating, setCreating] = useState(false);
  const [importing, setImporting] = useState(false);
  const [view, setView] = useState<{ name: string; agent?: AgentKind } | null>(null);
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
  const remove = async (s: SkillRow) => {
    const ok = await confirmDialog({ title: t('skills.delete'), message: t('skills.deleteAsk', { name: s.name }), confirmLabel: t('skills.deleteConfirm'), danger: true });
    if (ok) await act(`${s.name}:delete`, () => api.skillDelete(s.name, p));
  };
  const rows = (data?.skills ?? []).filter((s) => matches(query, s.name, s.description));

  return (
    <ExtPage
      title={t('skills.title')}
      intro={t('skills.intro')}
      help={skillsHelp()}
      project={project}
      onProject={setProject}
      query={query}
      onQuery={setQuery}
      searchPlaceholder={t('skills.search')}
      error={error}
      loading={!data && loading}
      actions={
        <>
          <Button size="sm" variant="primary" onClick={() => setImporting(true)}>
            <GitBranch className="size-3.5" /> {t('skills.import')}
          </Button>
          <Button size="sm" onClick={() => setCreating(true)}>
            <Plus className="size-3.5" /> {t('skills.new')}
          </Button>
        </>
      }
      footer={data ? <SkillDirs agents={data.agents} /> : null}
    >
      {data ? (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[600px] text-sm">
            <thead>
              <tr className="border-b border-line/60 text-left text-xs text-muted">
                <th className="px-4 py-2 font-normal sm:px-5">{t('skills.skill')}</th>
                <AgentHeads agents={data.agents} />
                <th className="w-10" />
              </tr>
            </thead>
            <tbody className="divide-y divide-line/50">
              {rows.map((s) => (
                <tr key={s.name} className="align-top">
                  <td className="max-w-sm px-4 py-2.5 sm:px-5">
                    <button type="button" onClick={() => setView({ name: s.name })} className="font-medium break-all hover:text-accent hover:underline">
                      {s.name}
                    </button>
                    {s.description ? <p className="mt-0.5 line-clamp-2 text-xs text-muted" title={s.description}>{s.description}</p> : null}
                  </td>
                  {data.agents.map((a) => (
                    <td key={a.kind} className="px-2 py-2.5 text-center">
                      <Cell s={s} a={a} busy={busy === `${s.name}:${a.kind}`} act={act} project={p} onView={() => setView({ name: s.name, agent: a.kind })} />
                    </td>
                  ))}
                  <td className="py-2 pr-3 text-right">
                    <RowMenu
                      label={t('common.actions')}
                      items={[
                        { label: t('skills.view'), icon: <Eye className="size-3.5" />, onClick: () => setView({ name: s.name }) },
                        {
                          label: t('skills.delete'),
                          icon: <Trash2 className="size-3.5" />,
                          danger: true,
                          disabled: !s.entries.some((e) => e.scope === 'user'),
                          onClick: () => void remove(s),
                        },
                      ]}
                    />
                  </td>
                </tr>
              ))}
              {!data.skills.length ? <EmptyRow cols={data.agents.length + 2} text={t('skills.empty')} /> : !rows.length ? <EmptyRow cols={data.agents.length + 2} text={t('ext.noMatch')} /> : null}
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
          <SkillViewer target={view} project={p} onClose={() => setView(null)} />
          <Sheet open={creating} onClose={() => setCreating(false)} title={t('skills.new')} wide>
            {creating ? (
              <NewSkill
                agents={data.agents}
                project={p}
                onDone={(r) => {
                  setData(r);
                  setCreating(false);
                }}
              />
            ) : null}
          </Sheet>
          <Sheet open={importing} onClose={() => setImporting(false)} title={t('skills.import')} wide>
            {importing ? (
              <ImportSkills
                agents={data.agents}
                project={p}
                onDone={(r) => {
                  setData(r);
                  setImporting(false);
                }}
              />
            ) : null}
          </Sheet>
        </>
      ) : null}
    </ExtPage>
  );
}

function SkillDirs({ agents }: { agents: SkillAgentInfo[] }) {
  return (
    <section className="rounded-2xl border border-line bg-surface px-4 py-3 sm:px-5">
      <h3 className="mb-2 text-sm font-semibold">{t('skills.dirs')}</h3>
      <ul className="space-y-1.5">
        {agents.map((a) => (
          <li key={a.kind} className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-sm">
            <AgentIcon kind={a.kind} className="size-4" />
            <span className="w-28 shrink-0">{a.label}</span>
            <code className="min-w-0 flex-1 truncate font-mono text-xs text-muted" title={a.dir ?? ''}>
              {a.dir ?? t('mcp.unsupported')}
            </code>
          </li>
        ))}
      </ul>
      <p className="mt-2 text-xs text-muted">{t('skills.trashHint')}</p>
    </section>
  );
}

function Cell({ s, a, busy, act, project, onView }: { s: SkillRow; a: SkillAgentInfo; busy: boolean; act: Act; project?: string; onView: () => void }) {
  const state = s.agents[a.kind] ?? null;
  if (state === 'project' || state === 'plugin') {
    const entry = s.entries.find((e) => e.agent === a.kind && e.scope === state);
    return (
      <button type="button" onClick={onView} title={t('ext.readOnlyAt', { path: entry?.path ?? '' })}>
        <ScopeBadge scope={state} />
      </button>
    );
  }
  const on = state === 'user';
  return (
    <span className="inline-flex justify-center">
      <Toggle
        on={on}
        disabled={busy || !a.supported}
        title={!a.supported ? t('skills.unsupported') : on ? t('skills.disconnect') : t('skills.connect')}
        label={`${s.name} · ${a.label}`}
        onClick={() => void act(`${s.name}:${a.kind}`, () => (on ? api.skillDisconnect(s.name, a.kind, project) : api.skillConnect(s.name, a.kind, project)))}
      />
    </span>
  );
}

function SkillViewer({ target, project, onClose }: { target: { name: string; agent?: AgentKind } | null; project?: string; onClose: () => void }) {
  const { data, loading, error } = useApi<SkillContent>(['harness-skill', target?.name, target?.agent, project], () => api.skill(target!.name, target!.agent, project), {
    enabled: target !== null,
  });
  const shown = data && target && data.name === target.name ? data : undefined;
  return (
    <FileViewer
      open={target !== null}
      title={target ? `${target.name} · SKILL.md` : ''}
      path={shown?.path}
      content={shown?.content}
      loading={loading && !shown}
      error={error}
      note={t('skills.viewNote')}
      onClose={onClose}
      aside={
        shown && shown.files.length > 1 ? (
          <div>
            <div className="mb-1 text-xs font-medium text-muted">{t('skills.files')}</div>
            <ul className="font-mono text-[11.5px] text-muted">
              {shown.files.map((f) => (
                <li key={f.path} className="flex gap-3">
                  <span className="min-w-0 flex-1 truncate">{f.path}</span>
                  <span>{f.size < 1024 ? `${f.size} B` : `${(f.size / 1024).toFixed(1)} KB`}</span>
                </li>
              ))}
            </ul>
          </div>
        ) : null
      }
    />
  );
}

function AgentPick({ agents, picked, setPicked }: { agents: SkillAgentInfo[]; picked: AgentKind[]; setPicked: (f: (cur: AgentKind[]) => AgentKind[]) => void }) {
  return (
    <div>
      <div className="mb-1.5 text-xs text-muted">{t('skills.agents')}</div>
      <div className="flex flex-wrap gap-x-4 gap-y-2">
        {agents.map((a) => (
          <span key={a.kind} title={a.supported ? (a.dir ?? '') : t('skills.unsupported')}>
            <Checkbox checked={picked.includes(a.kind) && a.supported} disabled={!a.supported} onChange={(v) => setPicked((cur) => (v ? [...cur, a.kind] : cur.filter((k) => k !== a.kind)))}>
              <span className="inline-flex items-center gap-1.5 text-sm">
                <AgentIcon kind={a.kind} className="size-4" /> {a.label}
              </span>
            </Checkbox>
          </span>
        ))}
      </div>
    </div>
  );
}

/** Run a write; a 409 (a different skill of that name) asks whether to replace it. */
async function withReplace(run: (replace: boolean) => Promise<SkillsResponse>): Promise<SkillsResponse | null> {
  try {
    return await run(false);
  } catch (e) {
    if (!isConflict(e)) throw e;
    const ok = await confirmDialog({ title: t('skills.replaceTitle'), message: `${errText(e)}\n\n${t('skills.replaceAsk')}`, confirmLabel: t('skills.replace'), danger: true });
    return ok ? run(true) : null;
  }
}

function NewSkill({ agents, project, onDone }: { agents: SkillAgentInfo[]; project?: string; onDone: (r: SkillsResponse) => void }) {
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [body, setBody] = useState('');
  const [picked, setPicked] = useState<AgentKind[]>(agents.filter((a) => a.supported).map((a) => a.kind));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async () => {
    setError(null);
    setBusy(true);
    try {
      const kinds = picked.filter((k) => agents.find((a) => a.kind === k)?.supported);
      const r = await withReplace((replace) => api.skillCreate({ name: name.trim(), description, body, agents: kinds, replace }, project));
      if (r) onDone(r);
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
      <Field label={t('skills.name')} hint={t('skills.nameHint')}>
        <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} placeholder="release-notes" autoFocus />
      </Field>
      <Field label={t('skills.description')} hint={t('skills.descriptionHint')}>
        <textarea className={`${inputCls} min-h-14`} rows={2} value={description} onChange={(e) => setDescription(e.target.value)} placeholder={t('skills.descriptionPlaceholder')} />
      </Field>
      <Field label={t('skills.body')} hint={t('skills.bodyHint')}>
        <textarea className={`${inputCls} min-h-48 font-mono text-[13px]`} rows={12} value={body} onChange={(e) => setBody(e.target.value)} placeholder={t('skills.bodyPlaceholder')} spellCheck={false} />
      </Field>
      <AgentPick agents={agents} picked={picked} setPicked={setPicked} />
      {error ? <ErrorLine error={error} /> : null}
      <Button type="submit" size="sm" variant="primary" loading={busy}>
        {t('skills.create')}
      </Button>
    </form>
  );
}

function ImportSkills({ agents, project, onDone }: { agents: SkillAgentInfo[]; project?: string; onDone: (r: SkillsResponse) => void }) {
  const [url, setUrl] = useState('');
  const [preview, setPreview] = useState<SkillImportPreview | null>(null);
  const [chosen, setChosen] = useState<string[]>([]);
  const [picked, setPicked] = useState<AgentKind[]>(agents.filter((a) => a.supported).map((a) => a.kind));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lookUp = async () => {
    setError(null);
    setBusy(true);
    try {
      const r = await api.skillImportPreview(url.trim());
      setPreview(r);
      setChosen([]);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  const run = async () => {
    if (!preview) return;
    setError(null);
    setBusy(true);
    try {
      const kinds = picked.filter((k) => agents.find((a) => a.kind === k)?.supported);
      const r = await withReplace((replace) => api.skillImport({ id: preview.id, skills: chosen, agents: kinds, replace }, project));
      if (r) onDone(r);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  if (!preview)
    return (
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          void lookUp();
        }}
      >
        <p className="text-sm text-muted">{t('skills.importHint')}</p>
        <Field label={t('skills.repo')} hint={t('skills.repoHint')}>
          <input className={`${inputCls} font-mono`} value={url} onChange={(e) => setUrl(e.target.value)} placeholder="anthropics/skills" autoFocus />
        </Field>
        {error ? <ErrorLine error={error} /> : null}
        <Button type="submit" size="sm" variant="primary" loading={busy} disabled={!url.trim()}>
          {busy ? t('skills.cloning') : t('skills.lookUp')}
        </Button>
      </form>
    );
  const all = preview.skills.map((s) => s.name);
  return (
    <form
      className="space-y-3"
      onSubmit={(e) => {
        e.preventDefault();
        void run();
      }}
    >
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted">{preview.url}</span>
        <button type="button" className="text-xs text-accent hover:underline" onClick={() => setChosen(chosen.length === all.length ? [] : all)}>
          {chosen.length === all.length ? t('skills.selectNone') : t('skills.selectAll')}
        </button>
      </div>
      <ul className="max-h-[45dvh] divide-y divide-line/60 overflow-y-auto border border-line/60">
        {preview.skills.map((s) => (
          <li key={s.name} className="px-3 py-2">
            <Checkbox checked={chosen.includes(s.name)} onChange={(v) => setChosen((cur) => (v ? [...cur, s.name] : cur.filter((n) => n !== s.name)))}>
              <span className="text-sm font-medium">{s.name}</span>
              <span className="ml-2 font-mono text-[11px] text-muted">{s.path}</span>
              {s.description ? <span className="mt-0.5 line-clamp-2 block text-xs text-muted">{s.description}</span> : null}
            </Checkbox>
          </li>
        ))}
      </ul>
      <AgentPick agents={agents} picked={picked} setPicked={setPicked} />
      {error ? <ErrorLine error={error} /> : null}
      <div className="flex gap-2">
        <Button type="submit" size="sm" variant="primary" loading={busy} disabled={!chosen.length}>
          {t('skills.importN', { n: chosen.length })}
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={() => setPreview(null)}>
          {t('skills.otherRepo')}
        </Button>
      </div>
    </form>
  );
}
