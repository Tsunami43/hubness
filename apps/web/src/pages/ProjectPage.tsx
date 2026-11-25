import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { Link, Navigate, useNavigate, useParams, useSearchParams } from 'react-router';
import { AppWindow, Boxes, ExternalLink, GitBranch, GitPullRequest, KeyRound, MessagesSquare, Play, Plus, Settings2, SquareTerminal, Trash2, X } from 'lucide-react';
import type { ProjectDetail, RunState } from '@hubness/shared';
import { api } from '../lib/api';
import { useApi } from '../lib/query';
import { RepoWorkspace, useGitByRepo, type Tool } from '../components/RepoWorkspace';
import { SessionsTool } from '../components/SessionsTool';
import { ProviderIcon } from '../components/ProviderIcon';
import { LanguageBar } from '../components/LanguageBar';
import { RunButton, RunPanel, useRuns } from '../components/project/runs';
import { PreviewPanel } from '../components/project/preview';
import { ptySize, ShellPanel } from '../components/project/shell';
import { EnvironmentSection } from '../components/project/environment';
import { SecretsPanel } from '../components/project/secrets';
import { Button, ErrorLine, Field, inputCls, Spinner } from '../components/ui';
import { Select } from '../components/Select';
import { confirmDialog } from '../components/Confirm';
import { t } from '../i18n';
import { closePlace, usePlace } from '../lib/places';

/** Project = one git repository: code and tools (Sessions, Git, Run, Preview, Shell, Secrets, Settings) as tabs. */
export function ProjectPage() {
  const { id = '' } = useParams();
  const [params, setParams] = useSearchParams();
  const { data: project, error, reload } = useApi(['project', id], () => api.project(id));
  const [focus, setFocus] = useState<string | null>(null);
  // Run state is polled often only while a Run/Preview tab is open
  const [openTools, setOpenTools] = useState<string[]>([]);
  const { runs, reload: reloadRuns } = useRuns(id, openTools.includes('run') || openTools.includes('preview'));
  const repos = useMemo(() => (project ? [{ id: project.id, name: project.name, path: project.path }] : []), [project?.id, project?.name, project?.path]);
  const { git, refresh: refreshGit } = useGitByRepo(repos);
  usePlace(project && project.id === id ? { key: `project:${id}`, kind: 'project', title: project.name, path: `/projects/${id}` } : null);
  // a deleted project should not linger in the top bar tabs
  useEffect(() => {
    if (error) closePlace(`project:${id}`);
  }, [error, id]);

  if (error)
    return (
      <div className="p-4">
        <ErrorLine error={error} />
      </div>
    );
  if (!project)
    return (
      <div className="flex justify-center py-10">
        <Spinner />
      </div>
    );

  const goto = (t: string) => {
    const next = new URLSearchParams(params);
    next.set('t', t);
    setParams(next, { replace: true });
  };
  const run = (r: RunState) =>
    void api.startRun(project.id, r.id, ptySize()).then(() => {
      setFocus(r.id);
      void reloadRuns();
      goto('run');
    });
  const normal = project.kind === 'normal';
  const repo = project.repo;

  const tools: Tool[] = [
    { id: 'sessions', label: t('tab.sessions'), icon: MessagesSquare, render: () => <SessionsTool scope={{ projectId: project.id }} root={project.path} title={project.name} /> },
    ...(normal
      ? [
          {
            id: 'run',
            label: t('run.title'),
            icon: Play,
            badge: runs?.some((r) => r.job?.status === 'running') ? ('dot' as const) : null,
            render: () => <RunPanel projectId={project.id} runs={runs} reload={() => void reloadRuns()} focus={focus} setFocus={setFocus} onPreview={() => goto('preview')} />,
          },
          { id: 'preview', label: t('preview.title'), icon: AppWindow, render: () => <PreviewPanel projectId={project.id} runs={runs} onStartRun={run} /> },
        ]
      : []),
    { id: 'shell', label: t('shell.title'), icon: SquareTerminal, render: () => <ShellPanel scope={{ projectId: project.id }} /> },
    ...(normal ? [{ id: 'secrets', label: t('secrets.title'), icon: KeyRound, render: () => <SecretsPanel projectId={project.id} /> }] : []),
    { id: 'settings', label: t('ws.settings'), icon: Settings2, render: () => <ProjectSettings project={project} onSaved={() => void reload()} /> },
  ];

  return (
    <RepoWorkspace
      storageKey={project.id}
      title={project.name}
      badge={project.kind === 'system' ? t('projects.kind.system') : undefined}
      subtitle={
        repo.isRepo ? (
          <span className="inline-flex items-center gap-1">
            <GitBranch className="size-3" />
            {repo.branch ?? 'detached'}
            {repo.providerLabel ? (
              <span className="inline-flex items-center gap-1">
                · <ProviderIcon provider={repo.provider} url={repo.remoteUrl} className="size-3" /> {repo.providerLabel}
              </span>
            ) : null}
          </span>
        ) : (
          <span>{project.path}</span>
        )
      }
      repos={repos}
      git={git}
      refreshGit={refreshGit}
      tools={tools}
      sessions={{ scope: { projectId: project.id }, root: project.path, title: project.name }}
      onOpenTools={setOpenTools}
      actions={normal ? <RunButton runs={runs} onOpen={() => goto('run')} onRun={run} onStop={(r) => void api.stopRun(project.id, r.id).then(() => void reloadRuns())} /> : null}
    />
  );
}

/** Project Settings tab: description, AI instructions, repository, groups, environment, deletion. */
function ProjectSettings({ project, onSaved }: { project: ProjectDetail; onSaved: () => void }) {
  const [name, setName] = useState(project.name);
  const [description, setDescription] = useState(project.description);
  const [instructions, setInstructions] = useState(project.instructions);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const nav = useNavigate();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  useEffect(() => {
    setName(project.name);
    setDescription(project.description);
    setInstructions(project.instructions);
    setConfirmRemove(false);
    setError(null);
  }, [project]);
  const dirty = name !== project.name || description !== project.description || instructions !== project.instructions;

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.updateProject(project.id, { name, description, instructions });
      onSaved();
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const r = project.repo;
  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-3">
      <div className="mx-auto max-w-2xl space-y-5">
        <Section title={t('projects.settingsTitle')}>
        <Field label={t('common.name')}>
          <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <Field label={t('projects.description')}>
          <textarea className={`${inputCls} min-h-16`} value={description} onChange={(e) => setDescription(e.target.value)} placeholder={t('projects.descriptionPlaceholder')} />
        </Field>
        <Field label={t('projects.instructions')} hint={t('projects.instructionsHint')}>
          <textarea className={`${inputCls} min-h-24 font-mono text-sm`} value={instructions} onChange={(e) => setInstructions(e.target.value)} />
        </Field>
        <div className="flex items-center gap-2">
          <Button size="sm" variant="primary" loading={busy} disabled={!name.trim() || !dirty} onClick={save}>
            {t('common.save')}
          </Button>
          {saved ? <span className="text-xs text-ok">{t('common.saved')}</span> : null}
        </div>
        </Section>

        <Section title={t('projects.repository')}>
          <dl className="divide-y divide-line border border-line text-sm">
            <Row label={t('common.folder')} value={<span className="font-mono text-xs break-all">{project.path}</span>} />
            <Row
              label={t('projects.provider')}
              value={
                <span className="inline-flex items-center gap-1.5">
                  <ProviderIcon provider={r.provider} url={r.remoteUrl} />
                  {r.providerLabel ?? t('projects.noRemote')}
                </span>
              }
            />
            {r.remoteUrl ? <Row label="origin" value={<span className="font-mono text-xs break-all">{r.remoteUrl}</span>} /> : null}
            <Row label={t('projects.branch')} value={<span className="font-mono text-xs">{r.branch ?? '—'}</span>} />
            <Row label={t('projects.defaultBranch')} value={<span className="font-mono text-xs">{r.defaultBranch ?? '—'}</span>} />
          </dl>
          {r.languages.length ? (
            <div className="mt-3">
              <LanguageBar languages={r.languages} />
            </div>
          ) : null}
          {r.links ? (
            <div className="mt-2 flex flex-wrap gap-2">
              <a href={r.links.repo} target="_blank" rel="noopener" className="inline-flex items-center gap-1.5 border border-line px-2.5 py-1.5 text-sm hover:bg-sunken">
                <ProviderIcon provider={r.provider} url={r.remoteUrl} className="size-3.5" /> {t('projects.openOn', { provider: r.providerLabel ?? '' })}
                <ExternalLink className="size-3 text-muted" />
              </a>
              {r.links.newPullRequest ? (
                <a href={r.links.newPullRequest} target="_blank" rel="noopener" className="inline-flex items-center gap-1.5 border border-line px-2.5 py-1.5 text-sm hover:bg-sunken">
                  <GitPullRequest className="size-3.5" /> {r.provider === 'gitlab' ? t('projects.newMr') : t('projects.newPr')}
                </a>
              ) : null}
            </div>
          ) : null}
        </Section>

        <Section title={t('projects.inGroups')}>
          <ProjectGroups project={project} onChanged={onSaved} />
        </Section>
        {project.kind === 'normal' ? (
          <Section title={t('env.title')}>
            <EnvironmentSection projectId={project.id} />
          </Section>
        ) : null}
        {project.kind === 'normal' ? (
          <div className="border border-danger/40 p-3">
            <div className="flex flex-wrap items-center gap-2">
              <span className="min-w-0 flex-1 text-sm text-muted">{t('projects.removeHint')}</span>
              {confirmRemove ? (
                <Button
                  size="sm"
                  variant="danger"
                  onClick={() =>
                    api.removeProject(project.id).then(
                      () => {
                        closePlace(`project:${project.id}`);
                        nav('/projects', { replace: true });
                      },
                      (e) => setError((e as Error).message),
                    )
                  }
                >
                  {t('projects.removeYes')}
                </Button>
              ) : (
                <Button size="sm" variant="danger" onClick={() => setConfirmRemove(true)}>
                  <Trash2 className="size-4" /> {t('projects.remove')}
                </Button>
              )}
            </div>
            <DeleteWithFiles project={project} onError={setError} />
          </div>
        ) : null}
        <ErrorLine error={error} />
      </div>
    </div>
  );
}

/**
 * Delete the project together with its folder on disk. Two confirmations: the first names the
 * folder, the second lists what is not saved anywhere else (uncommitted files, unpushed
 * commits, stashes), read from git right before asking.
 */
function DeleteWithFiles({ project, onError }: { project: ProjectDetail; onError: (e: string | null) => void }) {
  const nav = useNavigate();
  const [busy, setBusy] = useState(false);
  const folder = project.path.split('/').filter(Boolean).pop() ?? project.path;
  const run = async () => {
    onError(null);
    const first = await confirmDialog({
      title: t('projects.deleteFilesTitle'),
      message: t('projects.deleteFilesConfirm1', { path: project.path }),
      confirmLabel: t('projects.deleteFilesNext'),
      danger: true,
    });
    if (!first) return;
    setBusy(true);
    try {
      const st = await api.gitStatus(project.id).catch(() => null);
      const risks = [
        st?.files.length ? t('projects.deleteRiskChanges', { n: st.files.length }) : '',
        st?.ahead ? t('projects.deleteRiskAhead', { n: st.ahead }) : '',
        st && st.branch && !st.upstream ? t('projects.deleteRiskNoUpstream', { branch: st.branch }) : '',
        st?.stashes ? t('projects.deleteRiskStashes', { n: st.stashes }) : '',
        project.repo.remoteUrl ? '' : t('projects.deleteRiskNoRemote'),
      ].filter(Boolean);
      const second = await confirmDialog({
        title: t('projects.deleteFilesTitle'),
        message: [
          t('projects.deleteFilesConfirm2', { name: folder }),
          risks.length ? `\n${t('projects.deleteRisks')}\n${risks.map((r) => `• ${r}`).join('\n')}` : '',
          `\n${t('projects.deleteRiskBranches')}`,
        ].join('\n'),
        confirmLabel: t('projects.deleteFilesYes'),
        danger: true,
      });
      if (!second) return;
      await api.deleteProjectWithFiles(project.id, folder);
      closePlace(`project:${project.id}`);
      nav('/projects', { replace: true });
    } catch (e) {
      onError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="mt-3 flex flex-wrap items-center gap-2 border-t border-danger/30 pt-3">
      <span className="min-w-0 flex-1 text-sm text-muted">{t('projects.deleteFilesHint')}</span>
      <Button size="sm" variant="danger" loading={busy} disabled={busy} onClick={() => void run()}>
        <Trash2 className="size-4" /> {t('projects.deleteFiles')}
      </Button>
    </div>
  );
}

/** Settings section: TUI-style heading. */
export function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="space-y-3">
      <h3 className="border-b border-line pb-1 text-xs tracking-wider text-accent uppercase">{title}</h3>
      {children}
    </section>
  );
}

function Row({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex items-baseline gap-3 px-3 py-2">
      <dt className="w-28 shrink-0 text-muted">{label}</dt>
      <dd className="min-w-0 flex-1">{value}</dd>
    </div>
  );
}

/** Old links /projects/:id/s/:sid → the project's Sessions tab. */
export function ProjectSessionPage() {
  const { id = '', sid = 'new' } = useParams();
  return <Navigate to={`/projects/${id}?t=sessions&s=${sid}`} replace />;
}

/** Groups of the project: open, remove from, add to an existing one or start a new one with it. */
function ProjectGroups({ project, onChanged }: { project: ProjectDetail; onChanged: () => void }) {
  const { data: all, reload } = useApi(['groups'], () => api.groups());
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const member = new Set(project.groups.map((g) => g.id));
  const others = (all ?? []).filter((g) => !member.has(g.id));

  const change = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
      await reload();
      onChanged();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const setMembers = (groupId: string, add: boolean) => {
    const g = (all ?? []).find((x) => x.id === groupId);
    if (!g) return;
    const ids = add ? [...g.projectIds, project.id] : g.projectIds.filter((id) => id !== project.id);
    void change(() => api.updateGroup(groupId, { projectIds: ids }));
  };

  return (
    <div className="space-y-2.5">
      {project.groups.length ? (
        <div className="flex flex-wrap gap-1.5">
          {project.groups.map((g) => (
            <span key={g.id} className="inline-flex items-center border border-line text-sm">
              <Link to={`/groups/${g.id}`} className="inline-flex items-center gap-1.5 py-1 pr-1.5 pl-2.5 hover:bg-sunken">
                <Boxes className="size-3.5 text-accent" /> {g.name}
              </Link>
              <button
                type="button"
                disabled={busy}
                title={t('projects.leaveGroup', { name: g.name })}
                onClick={() => setMembers(g.id, false)}
                className="inline-flex size-7 items-center justify-center text-muted hover:bg-sunken hover:text-danger"
              >
                <X className="size-3.5" />
              </button>
            </span>
          ))}
        </div>
      ) : (
        <p className="text-sm text-muted">{t('projects.noGroups')}</p>
      )}
      <div className="flex flex-wrap items-center gap-2">
        {others.length ? (
          <Select
            label={t('projects.addToGroup')}
            value=""
            onChange={(id) => id && setMembers(id, true)}
            options={[{ value: '', label: t('projects.addToGroup') }, ...others.map((g) => ({ value: g.id, label: g.name }))]}
            className="max-w-56"
          />
        ) : null}
        <form
          className="flex min-w-56 flex-1 gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            const n = name.trim();
            if (n) void change(() => api.createGroup({ name: n, projectIds: [project.id] })).then(() => setName(''));
          }}
        >
          <input className={inputCls} placeholder={t('projects.newGroupWith')} value={name} onChange={(e) => setName(e.target.value)} />
          <Button type="submit" size="sm" disabled={!name.trim() || busy}>
            <Plus className="size-3.5" /> {t('git.create')}
          </Button>
        </form>
      </div>
      <ErrorLine error={error} />
    </div>
  );
}
