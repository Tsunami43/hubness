import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router';
import { Check, Download, FolderGit2, GitFork, Plus, Search } from 'lucide-react';
import type { JobInfo } from '@hubness/shared';
import { api } from '../lib/api';
import { t } from '../i18n';
import { FolderPicker } from './FolderPicker';
import { TerminalSheet } from './Terminal';
import { useHubSettings } from '../lib/hubSettings';
import { Button, ErrorLine, Field, inputCls, Sheet, Spinner } from './ui';

type Mode = 'clone' | 'empty' | 'import';

/**
 * New project (as in Replit/Coder, the hub stores the projects):
 * - clone by URL or an empty repository → into the storage folder (the group's or the shared one);
 * - import repositories that already live on the server (files are not moved).
 * If opened from a group, the project is added to the group right away.
 */
export function NewProject({
  open,
  onClose,
  onCreated,
  group,
}: {
  open: boolean;
  onClose: () => void;
  onCreated: () => void;
  group?: { id: string; name: string; folder: string | null };
}) {
  const nav = useNavigate();
  const settings = useHubSettings();
  const storage = group?.folder ?? settings?.projectsRoot ?? '';
  const [mode, setMode] = useState<Mode>('clone');
  const [url, setUrl] = useState('');
  const [name, setName] = useState('');
  const [picked, setPicked] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [job, setJob] = useState<JobInfo | null>(null);
  const derived = url.trim().replace(/\/+$/, '').split(/[/:]/).pop()?.replace(/\.git$/, '') ?? '';
  const folder = (name.trim() || (mode === 'clone' ? derived : '')) || '…';

  useEffect(() => {
    if (!open) return;
    setError(null);
    setPicked([]);
  }, [open]);

  const done = (projectId?: string) => {
    onCreated();
    onClose();
    setUrl('');
    setName('');
    if (projectId && !group) nav(`/projects/${projectId}`);
  };

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      if (mode === 'empty') {
        const p = await api.newRepo({ name, groupId: group?.id });
        done(p.id);
      } else if (mode === 'import') {
        const list = await api.importProjects({ paths: picked, groupId: group?.id });
        done(list.length === 1 ? list[0]!.id : undefined);
      } else {
        const w = Math.min(window.innerWidth, 768) - 56;
        setJob(await api.cloneRepo({ url: url.trim(), name: name.trim() || undefined, groupId: group?.id, cols: Math.max(40, Math.floor(w / 7.9)), rows: 20 }));
      }
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const can = mode === 'clone' ? Boolean(url.trim()) : mode === 'empty' ? Boolean(name.trim()) : picked.length > 0;
  const label = mode === 'clone' ? t('projects.clone') : mode === 'empty' ? t('projects.create') : t('projects.import', { n: picked.length });

  return (
    <>
      <Sheet
        open={open && !job}
        onClose={onClose}
        title={group ? t('groups.addProject') : t('projects.newTitle')}
        footer={
          <Button className="flex-1" variant="primary" loading={busy} disabled={!can} onClick={submit}>
            {label}
          </Button>
        }
      >
        <div className="space-y-4">
          <div className="flex rounded-lg bg-sunken p-0.5 text-sm">
            {(['clone', 'empty', 'import'] as const).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setMode(m)}
                className={`flex min-w-0 flex-1 items-center justify-center gap-1.5 rounded-md px-2 py-1.5 ${mode === m ? 'bg-surface font-medium text-fg' : 'text-muted hover:text-fg'}`}
              >
                {m === 'clone' ? <GitFork className="size-3.5 shrink-0" /> : m === 'empty' ? <Plus className="size-3.5 shrink-0" /> : <Download className="size-3.5 shrink-0" />}
                <span className="truncate">{m === 'clone' ? t('projects.modeClone') : m === 'empty' ? t('projects.modeEmpty') : t('projects.modeImport')}</span>
              </button>
            ))}
          </div>

          {mode === 'import' ? (
            <ImportPicker open={open} root={storage} value={picked} onChange={setPicked} />
          ) : (
            <>
              {mode === 'clone' ? (
                <Field label={t('projects.repoUrl')} hint={t('projects.repoUrlHint')}>
                  <input
                    autoFocus
                    className={`${inputCls} font-mono text-sm`}
                    value={url}
                    onChange={(e) => setUrl(e.target.value)}
                    placeholder="git@github.com:acme/auth-service.git"
                    autoCapitalize="off"
                    autoCorrect="off"
                    spellCheck={false}
                  />
                </Field>
              ) : null}
              <Field label={mode === 'clone' ? t('projects.folderName') : t('common.name')} hint={t('projects.newHint', { path: `${storage}/${folder}` })}>
                <input
                  className={inputCls}
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder={mode === 'clone' ? derived : ''}
                  onKeyDown={(e) => e.key === 'Enter' && can && void submit()}
                />
              </Field>
            </>
          )}
          {group ? <p className="text-xs text-muted">{t('projects.inGroup', { group: group.name })}</p> : null}
          <ErrorLine error={error} />
        </div>
      </Sheet>
      <TerminalSheet
        job={job}
        onClose={() => {
          setJob(null);
          done();
        }}
        onDone={onCreated}
      />
    </>
  );
}

/** Import: repositories from the storage folder not yet added to the hub, or from any other folder. */
function ImportPicker({ open, root, value, onChange }: { open: boolean; root: string; value: string[]; onChange: (paths: string[]) => void }) {
  const [where, setWhere] = useState<string | undefined>(undefined);
  const [browsing, setBrowsing] = useState(false);
  const [browsed, setBrowsed] = useState<{ path: string; isGit: boolean } | null>(null);
  const [data, setData] = useState<{ root: string; repos: { path: string; name: string }[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [q, setQ] = useState('');

  useEffect(() => {
    if (!open) return;
    setData(null);
    setError(null);
    api.discoverRepos(where ?? (root || undefined)).then(setData, (e) => setError((e as Error).message));
  }, [open, where, root]);

  const set = new Set(value);
  const toggle = (p: string) => onChange(set.has(p) ? value.filter((x) => x !== p) : [...value, p]);
  const needle = q.trim().toLowerCase();
  const list = data ? (needle ? data.repos.filter((r) => r.path.toLowerCase().includes(needle)) : data.repos) : [];
  const relOf = (p: string) => (data && p.startsWith(data.root + '/') ? p.slice(data.root.length + 1) : p);

  return (
    <div className="space-y-3">
      <p className="text-sm text-muted">{t('projects.importHint')}</p>
      <div>
        <div className="mb-1.5 flex items-center gap-2 text-sm">
          <span className="min-w-0 flex-1 truncate text-muted">{t('projects.importFound', { root: data?.root ?? where ?? root })}</span>
          <Button size="sm" variant="ghost" onClick={() => setBrowsing(!browsing)}>
            {browsing ? t('common.cancel') : t('projects.importOther')}
          </Button>
        </div>
        {browsing ? (
          <div className="mb-3 space-y-2 rounded-xl border border-line p-3">
            <FolderPicker onPick={(path, isGit) => setBrowsed({ path, isGit })} />
            {browsed ? (
              browsed.isGit ? (
                <Button
                  className="w-full"
                  onClick={() => {
                    if (!set.has(browsed.path)) onChange([...value, browsed.path]);
                    setBrowsing(false);
                  }}
                >
                  {t('projects.importThis')}
                </Button>
              ) : (
                <Button
                  className="w-full"
                  onClick={() => {
                    setWhere(browsed.path);
                    setBrowsing(false);
                  }}
                >
                  {t('projects.lookHere')}
                </Button>
              )
            ) : null}
          </div>
        ) : null}
        <ErrorLine error={error} />
        <div className="overflow-hidden rounded-xl border border-line">
          {data && data.repos.length > 8 ? (
            <label className="flex items-center gap-2 border-b border-line bg-bg px-3">
              <Search className="size-4 shrink-0 text-muted" />
              <input value={q} onChange={(e) => setQ(e.target.value)} placeholder={t('projects.search')} className="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none" />
            </label>
          ) : null}
          {!data && !error ? (
            <div className="flex justify-center py-6">
              <Spinner />
            </div>
          ) : list.length ? (
            <ul className="max-h-72 overflow-y-auto p-1">
              {list.map((r) => {
                const on = set.has(r.path);
                return (
                  <li key={r.path}>
                    <button
                      type="button"
                      onClick={() => toggle(r.path)}
                      className={`flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-sm ${on ? 'bg-accent/10' : 'hover:bg-sunken'}`}
                    >
                      <span className={`flex size-4 shrink-0 items-center justify-center rounded border ${on ? 'border-accent bg-accent text-accent-fg' : 'border-line'}`}>
                        {on ? <Check className="size-3" /> : null}
                      </span>
                      <FolderGit2 className="size-4 shrink-0 text-accent" />
                      <span className="min-w-0 flex-1 truncate font-mono text-[13px]">{relOf(r.path)}</span>
                    </button>
                  </li>
                );
              })}
            </ul>
          ) : (
            <p className="px-3 py-4 text-sm text-muted">{t('projects.importNone')}</p>
          )}
        </div>
        {value.some((p) => !data?.repos.some((r) => r.path === p)) ? (
          <ul className="mt-2 space-y-1">
            {value
              .filter((p) => !data?.repos.some((r) => r.path === p))
              .map((p) => (
                <li key={p} className="flex items-center gap-2 text-sm">
                  <Check className="size-3.5 text-accent" />
                  <span className="min-w-0 flex-1 truncate font-mono text-xs">{p}</span>
                  <button type="button" className="text-xs text-muted hover:text-fg" onClick={() => toggle(p)}>
                    {t('common.delete')}
                  </button>
                </li>
              ))}
          </ul>
        ) : null}
      </div>
    </div>
  );
}
