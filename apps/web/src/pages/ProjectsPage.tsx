import { useMemo, useState } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router';
import { Boxes, Check, FolderGit2, Plus, Search, Trash2 } from 'lucide-react';
import type { Project } from '@hubness/shared';
import { api } from '../lib/api';
import { useApi } from '../lib/query';
import { timeAgo } from '../lib/format';
import { useHubSettings } from '../lib/hubSettings';
import { NewProject } from '../components/NewProject';
import { ProviderIcon } from '../components/ProviderIcon';
import { LanguageDot } from '../components/LanguageBar';
import { Button, Empty, ErrorLine, Field, IconButton, inputCls, Sheet, Spinner, TopBar } from '../components/ui';
import { t } from '../i18n';
import { Select } from '../components/Select';
import { useAccess } from '../components/AuthGate';

const rel = (root: string | null | undefined, p: string) => (root && p.startsWith(root + '/') ? p.slice(root.length + 1) : p);

const PROVIDER_LABEL: Record<string, string> = { github: 'GitHub', gitlab: 'GitLab', bitbucket: 'Bitbucket', generic: 'Git' };
/** owner/name from a remote URL (git@host:owner/name.git, https://host/owner/name). */
const slugOf = (url: string | null) => (url ? url.replace(/^.*[:/]([^/]+\/[^/]+?)(\.git)?$/, '$1') : null);

type Sort = 'updated' | 'name';

/**
 * Projects page, laid out like the repository list on GitHub: groups in a side column
 * (a full-width list on a phone), projects as a list — name, groups, description and a meta line
 * (language, host, folder, last update). Search, group and language filters, sorting.
 */
export function ProjectsPage() {
  const settings = useHubSettings();
  // adding projects and groups is for admins
  const { admin } = useAccess();
  const [params, setParams] = useSearchParams();
  const { data, loading, error, reload } = useApi(['projects', 'with-groups'], () => Promise.all([api.projects(), api.groups()]));
  const [sheet, setSheet] = useState<'repo' | 'group' | null>(null);
  const [q, setQ] = useState('');
  const groupFilter = params.get('group') ?? '';
  const langFilter = params.get('lang') ?? '';
  const sort: Sort = params.get('sort') === 'name' ? 'name' : 'updated';
  const setParam = (k: string, v: string) => {
    const next = new URLSearchParams(params);
    if (v) next.set(k, v);
    else next.delete(k);
    setParams(next, { replace: true });
  };

  const normal = useMemo(() => (data?.[0] ?? []).filter((p) => p.kind === 'normal'), [data]);
  const groups = data?.[1] ?? [];
  const groupName = new Map(groups.map((g) => [g.id, g.name]));
  const projectName = new Map(normal.map((p) => [p.id, p.name]));
  const languages = [...new Set(normal.map((p) => p.language?.name).filter(Boolean) as string[])].sort();
  const needle = q.trim().toLowerCase();
  const shown = normal
    .filter((p) => !needle || `${p.name} ${p.description} ${p.path} ${p.repoUrl ?? ''}`.toLowerCase().includes(needle))
    .filter((p) => !groupFilter || p.groupIds.includes(groupFilter))
    .filter((p) => !langFilter || p.language?.name === langFilter)
    .sort((a, b) => (sort === 'name' ? a.name.localeCompare(b.name) : b.updatedAt.localeCompare(a.updatedAt)));
  const filtered = Boolean(needle || groupFilter || langFilter);

  return (
    <div className="flex flex-col">
      <TopBar
        title={t('projects.title')}
        subtitle={settings?.projectsRoot ? <span className="hidden font-mono sm:inline">{settings.projectsRoot}</span> : undefined}
      />
      <div className="mx-auto grid w-full max-w-6xl gap-6 p-4 lg:grid-cols-[260px_1fr]">
        {/* groups: side column on desktop, a full-width list on a phone */}
        <aside className="min-w-0">
          <div className="mb-2 flex items-center gap-2">
            <h2 className="flex-1 text-sm font-semibold">{t('projects.tab.groups')}</h2>
            {admin ? (
              <Button size="sm" variant="ghost" onClick={() => setSheet('group')}>
                <Plus className="size-3.5" /> {t('groups.new')}
              </Button>
            ) : null}
          </div>
          {groups.length ? (
            // one group per line at full width: a long project list ends with … instead of running off the screen
            <ul className="space-y-2 lg:space-y-1">
              {groups.map((g) => (
                <li key={g.id} className="min-w-0">
                  <Link data-jump to={`/groups/${g.id}`} className="group flex items-start gap-2 border border-line bg-surface px-3 py-2 hover:border-accent/60 lg:border-transparent lg:bg-transparent lg:px-2 lg:hover:bg-sunken">
                    <Boxes className="mt-0.5 size-4 shrink-0 text-accent" />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-sm font-medium group-hover:text-accent">{g.name}</span>
                      <span className="block truncate text-xs text-muted">
                        {g.projectIds.length ? g.projectIds.map((id) => projectName.get(id) ?? '?').join(', ') : t('groups.noRepos')}
                      </span>
                    </span>
                  </Link>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-xs text-muted">{t('groups.emptyHint')}</p>
          )}
        </aside>

        <section className="min-w-0">
          <div className="flex flex-wrap items-center gap-2 border-b border-line pb-3">
            <label className="flex min-w-48 flex-1 items-center gap-2 border border-line bg-surface px-2.5 focus-within:border-accent">
              <Search className="size-4 shrink-0 text-muted" />
              <input value={q} onChange={(e) => setQ(e.target.value)} placeholder={t('projects.find')} className="min-w-0 flex-1 bg-transparent py-1.5 text-sm outline-none" />
            </label>
            {groups.length ? (
              <Select label={t('projects.tab.groups')} value={groupFilter} onChange={(v) => setParam('group', v)} className="max-w-48" options={[{ value: '', label: t('projects.allGroups') }, ...groups.map((g) => ({ value: g.id, label: g.name }))]} />
            ) : null}
            {languages.length > 1 ? (
              <Select label={t('projects.language')} value={langFilter} onChange={(v) => setParam('lang', v)} className="max-w-48" options={[{ value: '', label: t('projects.allLanguages') }, ...languages.map((l) => ({ value: l, label: l }))]} />
            ) : null}
            <Select
              label={t('projects.sort')}
              value={sort === 'name' ? 'name' : 'updated'}
              onChange={(v) => setParam('sort', v === 'name' ? 'name' : '')}
              options={[
                { value: 'updated', label: t('projects.sortUpdated') },
                { value: 'name', label: t('projects.sortName') },
              ]}
            />
            {admin ? (
              <Button variant="primary" size="sm" onClick={() => setSheet('repo')}>
                <Plus className="size-4" /> {t('projects.new')}
              </Button>
            ) : null}
          </div>
          <ErrorLine error={error} />
          {loading && !data ? (
            <div className="flex justify-center py-10">
              <Spinner />
            </div>
          ) : shown.length ? (
            <ul className="divide-y divide-line">
              {shown.map((p) => (p.missing ? <MissingRow key={p.id} project={p} onRemoved={() => void reload()} /> : <ProjectRow key={p.id} project={p} root={settings?.projectsRoot} groupName={groupName} />))}
            </ul>
          ) : (
            <Empty
              icon={<FolderGit2 className="size-8" />}
              title={filtered ? t('projects.noMatches') : t('projects.empty')}
              hint={filtered ? undefined : admin ? t('projects.emptyHint') : t('perm.noProjects')}
              action={
                filtered || !admin ? undefined : (
                  <Button variant="primary" onClick={() => setSheet('repo')}>
                    {t('projects.new')}
                  </Button>
                )
              }
            />
          )}
        </section>
      </div>
      <NewProject open={sheet === 'repo'} onClose={() => setSheet(null)} onCreated={() => void reload()} />
      <NewGroup open={sheet === 'group'} onClose={() => setSheet(null)} projects={normal} root={settings?.projectsRoot ?? ''} />
    </div>
  );
}

/** One project, GitHub repository-list style. */
function ProjectRow({ project: p, root, groupName }: { project: Project; root: string | null | undefined; groupName: Map<string, string> }) {
  const slug = slugOf(p.repoUrl);
  const folder = rel(root, p.path);
  return (
    <li className="py-4">
      <div className="flex flex-wrap items-center gap-2">
        <Link data-jump to={`/projects/${p.id}`} className="min-w-0 truncate text-lg font-semibold text-accent hover:underline">
          {p.name}
        </Link>
        {p.groupIds.map((id) => (
          <Link key={id} to={`/groups/${id}`} className="inline-flex items-center gap-1 border border-line px-1.5 text-[11px] text-muted hover:border-accent/60 hover:text-fg">
            <Boxes className="size-3" /> {groupName.get(id)}
          </Link>
        ))}
      </div>
      {p.description ? <p className="mt-1 max-w-3xl text-sm text-muted">{p.description}</p> : null}
      <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted">
        {p.language ? (
          <span className="inline-flex items-center gap-1.5">
            <LanguageDot color={p.language.color} />
            {p.language.name}
          </span>
        ) : null}
        {p.gitProvider ? (
          <span className="inline-flex min-w-0 items-center gap-1" title={p.repoUrl ?? undefined}>
            <ProviderIcon provider={p.gitProvider} url={p.repoUrl} className="size-3.5" />
            <span className="truncate">{slug ?? PROVIDER_LABEL[p.gitProvider]}</span>
          </span>
        ) : (
          <span>{t('projects.noRemote')}</span>
        )}
        <span className="inline-flex min-w-0 items-center gap-1 font-mono" title={p.path}>
          <FolderGit2 className="size-3.5 shrink-0" />
          <span className="truncate">{folder}</span>
        </span>
        <span>{t('projects.updated', { time: timeAgo(p.updatedAt) })}</span>
      </div>
    </li>
  );
}

/** A project whose folder is gone from disk: it cannot be opened, only removed from the hub. */
function MissingRow({ project, onRemoved }: { project: Project; onRemoved: () => void }) {
  const [error, setError] = useState<string | null>(null);
  return (
    <li className="flex items-center gap-3 px-4 py-2.5">
      <FolderGit2 className="size-4 shrink-0 text-muted" />
      <span className="min-w-0 flex-1">
        <span className="block truncate font-medium text-muted">{project.name}</span>
        <span className="block truncate font-mono text-xs text-warn">
          {t('projects.missing')} · {error ?? project.path}
        </span>
      </span>
      <IconButton label={t('projects.remove')} onClick={() => api.removeProject(project.id).then(onRemoved, (e) => setError((e as Error).message))}>
        <Trash2 className="size-4" />
      </IconButton>
    </li>
  );
}

/** Project picker (for a group): search + checkboxes. */
export function ProjectPicker({ projects, value, onChange }: { projects: Project[]; value: string[]; onChange: (ids: string[]) => void }) {
  const [q, setQ] = useState('');
  const needle = q.trim().toLowerCase();
  const list = needle ? projects.filter((p) => `${p.name} ${p.path}`.toLowerCase().includes(needle)) : projects;
  const set = new Set(value);
  return (
    <div className="overflow-hidden rounded-xl border border-line">
      <label className="flex items-center gap-2 border-b border-line bg-bg px-3">
        <Search className="size-4 shrink-0 text-muted" />
        <input value={q} onChange={(e) => setQ(e.target.value)} placeholder={t('projects.search')} className="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none" />
      </label>
      <ul className="max-h-72 overflow-y-auto p-1">
        {list.map((p) => {
          const on = set.has(p.id);
          return (
            <li key={p.id}>
              <button
                type="button"
                onClick={() => onChange(on ? value.filter((x) => x !== p.id) : [...value, p.id])}
                className={`flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-sm ${on ? 'bg-accent/10' : 'hover:bg-sunken'}`}
              >
                <span className={`flex size-4 shrink-0 items-center justify-center rounded border ${on ? 'border-accent bg-accent text-accent-fg' : 'border-line'}`}>
                  {on ? <Check className="size-3" /> : null}
                </span>
                <span className="min-w-0 flex-1 truncate">{p.name}</span>
                <span className="max-w-[45%] shrink-0 truncate font-mono text-xs text-muted">{p.path.split('/').slice(-2).join('/')}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function NewGroup({ open, onClose, projects, root }: { open: boolean; onClose: () => void; projects: Project[]; root: string }) {
  const nav = useNavigate();
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [ids, setIds] = useState<string[]>([]);
  const [folder, setFolder] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      const g = await api.createGroup({ name, description, folder: folder.trim() || null, projectIds: ids });
      onClose();
      setName('');
      setDescription('');
      setIds([]);
      setFolder('');
      nav(`/groups/${g.id}`);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title={t('groups.newTitle')}
      footer={
        <Button className="flex-1" variant="primary" loading={busy} disabled={!name.trim()} onClick={create}>
          {t('groups.create', { n: ids.length })}
        </Button>
      }
    >
      <div className="space-y-4">
        <Field label={t('common.name')}>
          <input autoFocus className={inputCls} value={name} onChange={(e) => setName(e.target.value)} placeholder={t('groups.namePlaceholder')} />
        </Field>
        <Field label={t('projects.description')}>
          <input className={inputCls} value={description} onChange={(e) => setDescription(e.target.value)} placeholder={t('groups.descriptionPlaceholder')} />
        </Field>
        <Field label={t('groups.members')} hint={t('groups.membersHint')}>
          <ProjectPicker projects={projects} value={ids} onChange={setIds} />
        </Field>
        <GroupFolderField value={folder} onChange={setFolder} root={root} />
        <ErrorLine error={error} />
      </div>
    </Sheet>
  );
}

/** Storage folder for the group's new projects (optional). */
export function GroupFolderField({ value, onChange, root }: { value: string; onChange: (v: string) => void; root: string }) {
  return (
    <Field label={t('groups.folder')} hint={t('groups.folderHint', { root })}>
      <input
        className={`${inputCls} font-mono text-sm`}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={root ? `${root}/work` : '/home/me/work'}
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
      />
    </Field>
  );
}
