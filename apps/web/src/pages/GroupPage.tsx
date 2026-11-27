import { useEffect, useMemo, useState } from 'react';
import { Link, Navigate, useNavigate, useParams } from 'react-router';
import { Boxes, FolderGit2, MessagesSquare, Plus, Server, Settings2, SquareTerminal, Trash2 } from 'lucide-react';
import type { GroupDetail, Project } from '@hubness/shared';
import { api } from '../lib/api';
import { useApi } from '../lib/query';
import { RepoWorkspace, useDesktop, useGitByRepo, type Tool } from '../components/RepoWorkspace';
import { MobileWorkspace, SimpleViewButton } from '../components/MobileWorkspace';
import { useLocalStorage } from '../lib/hooks';
import { SessionsTool } from '../components/SessionsTool';
import { Button, Empty, ErrorLine, Field, inputCls, Spinner } from '../components/ui';
import { NewProject } from '../components/NewProject';
import { ProviderIcon } from '../components/ProviderIcon';
import { ShellPanel } from '../components/project/shell';
import { SshBindingsPanel } from '../components/project/ssh';
import { useHubSettings } from '../lib/hubSettings';
import { GroupFolderField, ProjectPicker } from './ProjectsPage';
import { Section } from './ProjectPage';
import { t } from '../i18n';
import { closePlace, usePlace } from '../lib/places';
import { useAccess } from '../components/AuthGate';

const NO_REPOS: { id: string; name: string; path: string }[] = [];

/** Project group: code of all repositories and tools (Sessions, Git, Shell, Settings) as tabs. */
export function GroupPage() {
  const { id = '' } = useParams();
  const { data: group, error, reload } = useApi(['group', id], () => api.group(id));
  const [adding, setAdding] = useState(false);
  const repoKey = group?.projects.map((p) => p.id).join(',') ?? '';
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const repos = useMemo(() => (group ? group.projects.map((p) => ({ id: p.id, name: p.name, path: p.path })) : []), [repoKey]);
  const access = useAccess();
  const { git, refresh } = useGitByRepo(access.can('git') ? repos : NO_REPOS);
  // phone: the simple chat-first view unless the full workspace was asked for
  const desktop = useDesktop();
  const [full, setFull] = useLocalStorage('hub.mobile.full', false);
  usePlace(group && group.id === id ? { key: `group:${id}`, kind: 'group', title: group.name, path: `/groups/${id}` } : null);
  useEffect(() => {
    if (error) closePlace(`group:${id}`);
  }, [error, id]);

  if (error)
    return (
      <div className="p-4">
        <ErrorLine error={error} />
      </div>
    );
  if (!group)
    return (
      <div className="flex justify-center py-10">
        <Spinner />
      </div>
    );

  const tools: Tool[] = [
    { id: 'sessions', label: t('tab.sessions'), icon: MessagesSquare, render: () => <SessionsTool scope={{ groupId: group.id }} root={group.workspace.path} title={group.name} /> },
    ...(access.can('terminal') ? [{ id: 'shell', label: t('shell.title'), icon: SquareTerminal, render: () => <ShellPanel scope={{ groupId: group.id }} /> }] : []),
    // SSH and group settings are for admins
    ...(access.admin ? [{ id: 'ssh', label: 'SSH', icon: Server, render: () => <SshBindingsPanel scope={{ groupId: group.id }} /> }] : []),
    ...(access.admin
      ? [
          {
            id: 'settings',
            label: t('ws.settings'),
            icon: Settings2,
            render: () => <GroupSettings group={group} onSaved={() => void reload()} onAddProject={() => setAdding(true)} />,
          },
        ]
      : []),
  ];

  if (!desktop && !full)
    return (
      <>
        <MobileWorkspace
          title={group.name}
          badge={t('projects.kind.group')}
          scope={{ groupId: group.id }}
          root={group.workspace.path}
          repos={repos}
          git={git}
          refreshGit={refresh}
          tools={tools.filter((x) => x.id !== 'sessions')}
          onFull={() => setFull(true)}
        />
        <NewProject open={adding} onClose={() => setAdding(false)} onCreated={() => void reload()} group={group} />
      </>
    );

  return (
    <>
      <RepoWorkspace
        storageKey={`group:${group.id}`}
        actions={desktop ? undefined : <SimpleViewButton onClick={() => setFull(false)} />}
        title={group.name}
        badge={t('projects.kind.group')}
        subtitle={
          <span>
            {t('projects.repos', { n: group.projects.length })}
            {group.workspace.status !== 'ready' && group.workspace.statusDetail ? <span className="text-warn"> · {group.workspace.statusDetail}</span> : null}
          </span>
        }
        repos={repos}
        git={git}
        refreshGit={refresh}
        tools={tools}
        sessions={{ scope: { groupId: group.id }, root: group.workspace.path, title: group.name }}
        empty={
          <Empty
            icon={<Boxes className="size-8" />}
            title={t('groups.noRepos')}
            hint={t('groups.emptyRepos')}
            action={
              <Button variant="primary" onClick={() => setAdding(true)}>
                <Plus className="size-4" /> {t('groups.addProject')}
              </Button>
            }
          />
        }
      />
      {/* project creation is a dialog: it is an action, not a tool */}
      <NewProject open={adding} onClose={() => setAdding(false)} onCreated={() => void reload()} group={group} />
    </>
  );
}

/** Group Settings tab: description, shared instructions, members, storage folder, agent context, deletion. */
function GroupSettings({ group, onSaved, onAddProject }: { group: GroupDetail; onSaved: () => void; onAddProject: () => void }) {
  const nav = useNavigate();
  const hub = useHubSettings();
  const [folder, setFolder] = useState(group.folder ?? '');
  const [name, setName] = useState(group.name);
  const [description, setDescription] = useState(group.description);
  const [instructions, setInstructions] = useState(group.instructions);
  const [members, setMembers] = useState<string[]>(group.projectIds);
  const [all, setAll] = useState<Project[] | null>(null);
  const [context, setContext] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setName(group.name);
    setDescription(group.description);
    setInstructions(group.instructions);
    setMembers(group.projectIds);
    setFolder(group.folder ?? '');
    setConfirmDelete(false);
    setError(null);
    api.projects().then((list) => setAll(list.filter((p) => p.kind === 'normal')), () => {});
  }, [group]);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.updateGroup(group.id, { name, description, instructions, folder: folder.trim() || null, projectIds: members });
      onSaved();
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    try {
      await api.deleteGroup(group.id);
      nav('/projects?tab=groups', { replace: true });
    } catch (e) {
      setError((e as Error).message);
    }
  };

  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-3">
      <div className="mx-auto max-w-2xl space-y-5">
        <Section title={t('groups.settings')}>
          <Field label={t('common.name')}>
            <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
          <Field label={t('projects.description')}>
            <textarea className={`${inputCls} min-h-16`} value={description} onChange={(e) => setDescription(e.target.value)} placeholder={t('groups.descriptionPlaceholder')} />
          </Field>
          <Field label={t('groups.instructions')} hint={t('groups.instructionsHint')}>
            <textarea className={`${inputCls} min-h-24 text-sm`} value={instructions} onChange={(e) => setInstructions(e.target.value)} />
          </Field>
          <Field label={t('groups.members')} hint={t('groups.membersHint')}>
            {all ? <ProjectPicker projects={all} value={members} onChange={setMembers} /> : <Spinner />}
          </Field>
          <GroupFolderField value={folder} onChange={setFolder} root={hub?.projectsRoot ?? ''} />
          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" variant="primary" loading={busy} onClick={save} disabled={!name.trim()}>
              {t('common.save')}
            </Button>
            <Button size="sm" onClick={onAddProject}>
              <Plus className="size-3.5" /> {t('groups.addProject')}
            </Button>
            {saved ? <span className="text-xs text-ok">{t('common.saved')}</span> : null}
          </div>
        </Section>

        <Section title={t('groups.repositories')}>
          <ul className="divide-y divide-line border border-line">
            {group.projects.map((p) => (
              <li key={p.id}>
                <Link to={`/projects/${p.id}`} className="flex items-center gap-2 px-3 py-2 text-sm hover:bg-sunken">
                  {p.gitProvider ? <ProviderIcon provider={p.gitProvider} url={p.repoUrl} /> : <FolderGit2 className="size-4 shrink-0 text-accent" />}
                  <span className="min-w-0 flex-1 truncate">{p.name}</span>
                  <span className="shrink-0 text-xs text-muted">{p.repoUrl ? p.repoUrl.replace(/^.*[:/]([^/]+\/[^/]+?)(\.git)?$/, '$1') : t('projects.noRemote')}</span>
                </Link>
              </li>
            ))}
          </ul>
          <p className="text-xs break-all text-muted">{group.workspace.path}</p>
        </Section>

        <Section title={t('groups.context')}>
          <p className="text-xs text-muted">{t('groups.contextHint')}</p>
          <Button size="sm" onClick={() => api.groupContext(group.id).then((r) => setContext(r.context), (e) => setError((e as Error).message))}>
            {context ? t('files.refresh') : t('groups.showContext')}
          </Button>
          {context ? <pre className="max-h-80 overflow-auto border border-line bg-surface px-3 py-2 text-[12px] whitespace-pre-wrap">{context}</pre> : null}
        </Section>

        <div className="flex flex-wrap items-center gap-2 border border-danger/40 p-3">
          <span className="min-w-0 flex-1 text-sm text-muted">{t('groups.deleteHint')}</span>
          {confirmDelete ? (
            <Button size="sm" variant="danger" onClick={remove}>
              {t('groups.deleteConfirm')}
            </Button>
          ) : (
            <Button size="sm" variant="danger" onClick={() => setConfirmDelete(true)}>
              <Trash2 className="size-3.5" /> {t('common.delete')}
            </Button>
          )}
        </div>
        <ErrorLine error={error} />
      </div>
    </div>
  );
}

/** Old links /groups/:id/s/:sid → the group's Sessions tab. */
export function GroupSessionPage() {
  const { id = '', sid = 'new' } = useParams();
  return <Navigate to={`/groups/${id}?t=sessions&s=${sid}`} replace />;
}
