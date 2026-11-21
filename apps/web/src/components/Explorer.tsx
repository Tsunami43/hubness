import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  ChevronRight,
  ChevronsDownUp,
  Eye,
  EyeOff,
  FilePlus,
  FolderGit2,
  FolderPlus,
  RefreshCw,
  Search,
  X,
} from 'lucide-react';
import type { FsEntry, GitStatus } from '@hubness/shared';
import type { Repo } from '../lib/repo';
import { api } from '../lib/api';
import { useLocalStorage } from '../lib/hooks';
import { t } from '../i18n';
import { Button, ErrorLine, Spinner } from './ui';
import { FileIcon } from './FileIcon';
import { endDrag, getDrag, startDrag } from '../lib/dnd';
import { useWorkbench } from '../lib/workbench';
import { useFileChanges } from '../lib/fileWatch';

/**
 * Project file tree (like Explorer in VS Code / Zed): lazy folder loading,
 * git status for files and folders, git-ignored files can be hidden, search across
 * all files. In a group, one branch per repository.
 */

/** Selected file: repository (project id) and the path inside it. */
export interface Selection {
  repo: string;
  path: string;
}

/** git status letter for a file: M modified, A added, U new (untracked), D deleted, R renamed, ! conflict. */
function gitMarks(git: GitStatus | null | undefined) {
  const files = new Map<string, string>();
  for (const f of git?.files ?? []) {
    const l = f.conflicted ? '!' : f.untracked ? 'U' : f.index !== ' ' && f.index !== '?' ? f.index : f.worktree;
    files.set(f.path, l === '?' ? 'U' : l);
  }
  // folder: "modified" (yellow) if it contains changes; only new files means green
  const dirs = new Map<string, 'changed' | 'new'>();
  for (const [p, l] of files) {
    const parts = p.split('/');
    for (let i = 1; i < parts.length; i++) {
      const d = parts.slice(0, i).join('/');
      if (l !== 'U' && l !== 'A') dirs.set(d, 'changed');
      else if (!dirs.has(d)) dirs.set(d, 'new');
    }
  }
  return { files, dirs };
}

const MARK_COLOR: Record<string, string> = { M: 'text-warn', A: 'text-ok', U: 'text-ok', D: 'text-danger', R: 'text-warn', '!': 'text-danger' };

type Children = FsEntry[] | 'loading' | { error: string };

export function Explorer({
  storageKey,
  repos,
  gitByRoot,
  selected,
  onSelect,
  reloadKey,
  focusNonce,
}: {
  /** Key for remembering expanded folders (project or group). */
  storageKey: string;
  repos: Repo[];
  /** git status by repository id. */
  gitByRoot: Record<string, GitStatus | null>;
  selected: Selection | null;
  /** Single click — preview (pinned = false), double click — a regular tab. */
  onSelect: (s: Selection, pinned?: boolean) => void;
  /** When it changes, the file search box gets focus (Ctrl+P). */
  focusNonce?: number;
  /** When it changes, the tree is reloaded (after git operations etc.). */
  reloadKey?: number;
}) {
  const roots = repos;
  const multi = roots.length > 1;
  const [expandedList, setExpandedList] = useLocalStorage<string[]>(`hub.tree.${storageKey}`, multi ? roots.map((r) => `${r.id}:`) : []);
  const expanded = useMemo(() => new Set(expandedList), [expandedList]);
  const [showIgnored, setShowIgnored] = useLocalStorage('hub.tree.ignored', false);
  const [children, setChildren] = useState<Map<string, Children>>(new Map());
  const [query, setQuery] = useState('');
  const [searching, setSearching] = useState(false);
  const [enterTick, setEnterTick] = useState(0);
  const loadingRef = useRef(new Set<string>());
  const searchRef = useRef<HTMLInputElement>(null);
  const wb = useWorkbench();
  // file operations: the row under the keyboard (F2 / Delete), inline name input, menu, confirm
  const [cursor, setCursor] = useState<Entry | null>(null);
  const [edit, setEdit] = useState<{ type: 'new'; root: string; parent: string; dir: boolean } | { type: 'rename'; root: string; path: string; dir: boolean } | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; root: string; entry: Entry | null } | null>(null);
  const [confirmDel, setConfirmDel] = useState<Entry | null>(null);
  const [dropKey, setDropKey] = useState<string | null>(null);
  const [opError, setOpError] = useState<string | null>(null);
  useEffect(() => {
    if (!focusNonce) return;
    searchRef.current?.focus();
    searchRef.current?.select();
  }, [focusNonce]);

  const load = useCallback(
    (root: string, path: string, force = false) => {
      const key = `${root}:${path}`;
      if (loadingRef.current.has(key)) return;
      setChildren((m) => {
        if (!force && m.has(key) && m.get(key) !== 'loading') return m;
        loadingRef.current.add(key);
        api.tree(root, path).then(
          (list) => setChildren((x) => new Map(x).set(key, list)),
          (e) => setChildren((x) => new Map(x).set(key, { error: (e as Error).message })),
        ).finally(() => loadingRef.current.delete(key));
        return new Map(m).set(key, m.get(key) && force ? m.get(key)! : 'loading');
      });
    },
    [],
  );

  // roots and expanded folders
  useEffect(() => {
    if (!multi && roots[0]) load(roots[0].id, '');
    for (const k of expanded) {
      const i = k.indexOf(':');
      load(k.slice(0, i), k.slice(i + 1));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storageKey, expandedList]);

  // reload everything that is loaded
  const refresh = useCallback(() => {
    for (const k of children.keys()) {
      const i = k.indexOf(':');
      load(k.slice(0, i), k.slice(i + 1), true);
    }
  }, [children, load]);
  useEffect(() => {
    if (reloadKey) refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reloadKey]);

  // changes on disk: reload the loaded folders that contain a changed path
  useFileChanges((m) => {
    const dirs = m.overflow ? null : new Set(m.paths.map((p) => (p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : '')));
    for (const k of children.keys()) {
      const i = k.indexOf(':');
      const [root, path] = [k.slice(0, i), k.slice(i + 1)];
      if (root === m.projectId && (!dirs || dirs.has(path))) load(root, path, true);
    }
  });

  // expand the path to the selected file
  useEffect(() => {
    if (!selected) return;
    const parts = selected.path.split('/').slice(0, -1);
    const need = [...(multi ? [`${selected.repo}:`] : []), ...parts.map((_, i) => `${selected.repo}:${parts.slice(0, i + 1).join('/')}`)];
    if (need.some((k) => !expanded.has(k))) setExpandedList([...new Set([...expandedList, ...need])]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected?.repo, selected?.path]);

  const toggle = (key: string) => setExpandedList(expanded.has(key) ? expandedList.filter((k) => k !== key) : [...expandedList, key]);
  const marks = useMemo(() => Object.fromEntries(roots.map((r) => [r.id, gitMarks(gitByRoot[r.id])])), [roots, gitByRoot]);

  const run = async (fn: () => Promise<void>) => {
    setOpError(null);
    try {
      await fn();
    } catch (e) {
      setOpError((e as Error).message);
    }
  };
  /** New file/folder: in the folder under the cursor (or next to the file). */
  const startNew = (root: string, dir: boolean, at?: Entry | null) => {
    const base = at === undefined ? (cursor?.root === root ? cursor : null) : at;
    const parent = base ? (base.dir ? base.path : parentOf(base.path)) : '';
    const need = [...(multi ? [`${root}:`] : []), ...(parent ? parent.split('/').map((_, i, a) => `${root}:${a.slice(0, i + 1).join('/')}`) : [])];
    if (need.some((k) => !expanded.has(k))) setExpandedList([...new Set([...expandedList, ...need])]);
    setEdit({ type: 'new', root, parent, dir });
  };
  const commitNew = (root: string, parent: string, dir: boolean, name: string) =>
    run(async () => {
      const r = await api.fsCreate(root, joinPath(parent, name), dir);
      load(root, parent, true);
      if (!dir) onSelect({ repo: root, path: r.path }, true);
      setCursor({ root, path: r.path, dir });
    });
  const commitRename = (root: string, path: string, dir: boolean, name: string) =>
    run(async () => {
      const r = await api.fsRename(root, path, joinPath(parentOf(path), name));
      load(root, parentOf(path), true);
      wb?.renamed(root, path, r.path);
      setCursor({ root, path: r.path, dir });
    });
  const moveInto = (root: string, from: string, dir: string) => {
    const to = joinPath(dir, from.slice(from.lastIndexOf('/') + 1));
    if (to === from || dir === from || dir.startsWith(`${from}/`)) return;
    void run(async () => {
      const r = await api.fsRename(root, from, to);
      load(root, parentOf(from), true);
      load(root, dir, true);
      wb?.renamed(root, from, r.path);
    });
  };
  const doDelete = (e: Entry) =>
    run(async () => {
      await api.fsDelete(e.root, e.path);
      setConfirmDel(null);
      load(e.root, parentOf(e.path), true);
      wb?.deleted(e.root, e.path);
      if (cursor?.path === e.path) setCursor(null);
    });
  /** Drag over a folder (or a root): a file or folder from the same repository can move there. */
  const canDrop = (root: string) => {
    const d = getDrag();
    return (d?.kind === 'ref' && d.ref.kind === 'file' && d.ref.repo === root) || (d?.kind === 'dir' && d.repo === root);
  };
  const dropInto = (root: string, dir: string) => {
    const d = getDrag();
    endDrag();
    setDropKey(null);
    if (d?.kind === 'ref' && d.ref.kind === 'file') moveInto(root, d.ref.path, dir);
    else if (d?.kind === 'dir') moveInto(root, d.path, dir);
  };

  const renderDir = (root: string, path: string, depth: number): ReactNode => {
    const c = children.get(`${root}:${path}`);
    if (!c || c === 'loading')
      return (
        <div style={{ paddingLeft: 12 + depth * 14 }} className="py-1.5">
          <Spinner />
        </div>
      );
    if ('error' in c)
      return (
        <div style={{ paddingLeft: 12 + depth * 14 }} className="py-1 text-xs text-danger">
          {c.error}
        </div>
      );
    const list = showIgnored ? c : c.filter((e) => !e.ignored);
    const creating = edit?.type === 'new' && edit.root === root && edit.parent === path ? edit : null;
    const input = creating ? (
      <NameInput
        key="new"
        depth={depth}
        dir={creating.dir}
        initial=""
        onCancel={() => setEdit(null)}
        onCommit={(name) => {
          setEdit(null);
          void commitNew(root, path, creating.dir, name);
        }}
      />
    ) : null;
    if (!list.length)
      return (
        input ?? (
          <div style={{ paddingLeft: 28 + depth * 14 }} className="py-1 text-xs text-muted">
            {t('files.empty')}
          </div>
        )
      );
    return [input, ...list.map((e) => {
      const key = `${root}:${e.path}`;
      const open = e.dir && expanded.has(key);
      const m = marks[root];
      const mark = e.dir ? undefined : m?.files.get(e.path);
      const changedDir = e.dir ? m?.dirs.get(e.path) : undefined;
      const active = selected?.repo === root && selected.path === e.path;
      const entry: Entry = { root, path: e.path, dir: e.dir };
      if (edit?.type === 'rename' && edit.root === root && edit.path === e.path)
        return (
          <div key={key}>
            <NameInput
              depth={depth}
              dir={e.dir}
              initial={e.name}
              onCancel={() => setEdit(null)}
              onCommit={(name) => {
                setEdit(null);
                if (name !== e.name) void commitRename(root, e.path, e.dir, name);
              }}
            />
            {open ? renderDir(root, e.path, depth + 1) : null}
          </div>
        );
      return (
        <div key={key}>
          <button
            type="button"
            onClick={() => {
              if (e.dir && open) {
                // collapsing: the folder is no longer the target for "new file" — its parent is
                const up = parentOf(e.path);
                setCursor(up ? { root, path: up, dir: true } : null);
              } else setCursor(entry);
              if (e.dir) toggle(key);
              else onSelect({ repo: root, path: e.path }, false);
            }}
            onContextMenu={(ev) => {
              ev.preventDefault();
              ev.stopPropagation();
              setCursor(entry);
              setMenu({ x: ev.clientX, y: ev.clientY, root, entry });
            }}
            onDoubleClick={() => (e.dir ? undefined : onSelect({ repo: root, path: e.path }, true))}
            // a file is dragged into the editor (tab line / group edge) or onto a folder (move);
            // a folder — only onto another folder
            draggable
            onDragStart={(ev) => {
              ev.dataTransfer.effectAllowed = 'copyMove';
              ev.dataTransfer.setData('text/plain', e.path);
              startDrag(e.dir ? { kind: 'dir', repo: root, path: e.path } : { kind: 'ref', ref: { kind: 'file', repo: root, path: e.path } });
            }}
            onDragEnd={() => {
              endDrag();
              setDropKey(null);
            }}
            onDragOver={(ev) => {
              if (!e.dir || !canDrop(root)) return;
              ev.preventDefault();
              ev.stopPropagation();
              setDropKey(key);
            }}
            onDragLeave={() => dropKey === key && setDropKey(null)}
            onDrop={(ev) => {
              if (!e.dir) return;
              ev.preventDefault();
              ev.stopPropagation();
              dropInto(root, e.path);
            }}
            style={{ paddingLeft: 8 + depth * 14 }}
            className={`group flex w-full items-center gap-1.5 py-[5px] pr-2 text-left text-[13.5px] lg:py-[3px] ${
              dropKey === key ? 'bg-accent/25 outline outline-1 outline-accent' : active ? 'bg-accent/15 text-fg' : cursor?.root === root && cursor.path === e.path ? 'bg-sunken' : 'hover:bg-sunken'
            } ${e.ignored ? 'opacity-50' : ''}`}
          >
            {e.dir ? (
              <ChevronRight className={`size-3.5 shrink-0 text-muted transition-transform ${open ? 'rotate-90' : ''}`} />
            ) : (
              <span className="w-3.5 shrink-0" />
            )}
            <FileIcon name={e.name} dir={e.dir} open={open} />
            <span className={`min-w-0 flex-1 truncate ${mark ? MARK_COLOR[mark] : changedDir === 'changed' ? 'text-warn/90' : changedDir === 'new' ? 'text-ok/90' : ''}`}>{e.name}</span>
            {mark ? <span className={`shrink-0 font-mono text-[11px] font-semibold ${MARK_COLOR[mark]}`}>{mark}</span> : null}
            {changedDir ? <span className={`size-1.5 shrink-0 rounded-full ${changedDir === 'changed' ? 'bg-warn/80' : 'bg-ok/80'}`} /> : null}
          </button>
          {open ? renderDir(root, e.path, depth + 1) : null}
        </div>
      );
    })];
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-1 border-b border-line px-2 py-1.5">
        <label className="flex min-w-0 flex-1 items-center gap-1.5 rounded-lg bg-sunken px-2">
          <Search className="size-3.5 shrink-0 text-muted" />
          <input
            ref={searchRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onFocus={() => setSearching(true)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') {
                setQuery('');
                e.currentTarget.blur();
              }
              // Enter — open the first match
              if (e.key === 'Enter') setEnterTick((n) => n + 1);
            }}
            placeholder={t('files.search')}
            className="min-w-0 flex-1 bg-transparent py-1.5 text-sm outline-none"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
          />
          {query ? (
            <button type="button" onClick={() => setQuery('')} aria-label={t('common.close')} className="text-muted hover:text-fg">
              <X className="size-3.5" />
            </button>
          ) : null}
        </label>
        {roots.length ? (
          <>
            <ToolButton label={t('files.newFile')} onClick={() => startNew(cursor?.root ?? roots[0]!.id, false)}>
              <FilePlus className="size-4" />
            </ToolButton>
            <ToolButton label={t('files.newFolder')} onClick={() => startNew(cursor?.root ?? roots[0]!.id, true)}>
              <FolderPlus className="size-4" />
            </ToolButton>
          </>
        ) : null}
        <ToolButton label={showIgnored ? t('files.hideIgnored') : t('files.showIgnored')} onClick={() => setShowIgnored(!showIgnored)} active={showIgnored}>
          {showIgnored ? <Eye className="size-4" /> : <EyeOff className="size-4" />}
        </ToolButton>
        <ToolButton
          label={t('files.collapseAll')}
          onClick={() => {
            setExpandedList([]);
            // nothing stays selected in a collapsed tree: new files go to the root
            setCursor(null);
          }}
        >
          <ChevronsDownUp className="size-4" />
        </ToolButton>
        <ToolButton label={t('files.refresh')} onClick={refresh}>
          <RefreshCw className="size-4" />
        </ToolButton>
      </div>
      <div
        className="min-h-0 flex-1 overflow-y-auto py-1 outline-none"
        tabIndex={-1}
        // a click on the empty space below the tree drops the selection (new files go to the root)
        onClick={(e) => e.target === e.currentTarget && setCursor(null)}
        onKeyDown={(e) => {
          if (edit || !cursor || (e.target as HTMLElement).tagName === 'INPUT') return;
          if (e.key === 'F2') {
            e.preventDefault();
            setEdit({ type: 'rename', root: cursor.root, path: cursor.path, dir: cursor.dir });
          } else if (e.key === 'Delete') {
            e.preventDefault();
            setConfirmDel(cursor);
          }
        }}
        onContextMenu={(e) => {
          if (!roots[0] || multi) return;
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY, root: roots[0].id, entry: null });
        }}
        onDragOver={(e) => {
          if (multi || !roots[0] || !canDrop(roots[0].id)) return;
          e.preventDefault();
          setDropKey('root');
        }}
        onDragLeave={(e) => e.target === e.currentTarget && setDropKey(null)}
        onDrop={(e) => {
          if (multi || !roots[0]) return;
          e.preventDefault();
          dropInto(roots[0].id, '');
        }}
      >
        {query.trim() && searching ? (
          <FileSearch repos={repos} query={query.trim()} enterTick={enterTick} onSelect={(s, pinned) => onSelect(s, pinned)} />
        ) : multi ? (
          roots.map((r) => {
            const key = `${r.id}:`;
            const open = expanded.has(key);
            const g = gitByRoot[r.id];
            return (
              <div key={r.id}>
                <button
                  type="button"
                  onClick={() => toggle(key)}
                  className="flex w-full items-center gap-1.5 px-2 py-1.5 text-left text-[13px] font-semibold tracking-wide uppercase hover:bg-sunken"
                  title={r.path}
                >
                  <ChevronRight className={`size-3.5 shrink-0 text-muted transition-transform ${open ? 'rotate-90' : ''}`} />
                  <FolderGit2 className="size-4 shrink-0 text-accent" />
                  <span className="min-w-0 flex-1 truncate">{r.name}</span>
                  {g?.branch ? <span className="shrink-0 font-mono text-[11px] font-normal text-muted normal-case">{g.branch}</span> : null}
                  {g?.files.length ? <span className="shrink-0 rounded-full bg-warn/20 px-1.5 text-[11px] font-medium text-warn">{g.files.length}</span> : null}
                </button>
                {open ? renderDir(r.id, '', 1) : null}
              </div>
            );
          })
        ) : roots[0] ? (
          renderDir(roots[0].id, '', 0)
        ) : null}
      </div>
      {confirmDel ? (
        <div className="shrink-0 space-y-2 border-t border-danger/40 bg-danger/5 p-2 text-[13px]">
          <div className="break-all">
            {t('files.deleteConfirm', { name: confirmDel.path })}
            {confirmDel.dir ? <span className="text-muted"> {t('files.deleteFolderHint')}</span> : null}
          </div>
          <div className="flex gap-2">
            <Button size="sm" variant="danger" onClick={() => void doDelete(confirmDel)}>
              {t('common.delete')}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDel(null)}>
              {t('common.cancel')}
            </Button>
          </div>
        </div>
      ) : null}
      {opError ? (
        <div className="shrink-0 border-t border-line px-2 py-1">
          <ErrorLine error={opError} />
        </div>
      ) : null}
      {menu ? (
        <TreeMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={[
            { label: t('files.newFile'), run: () => startNew(menu.root, false, menu.entry) },
            { label: t('files.newFolder'), run: () => startNew(menu.root, true, menu.entry) },
            ...(menu.entry
              ? [
                  null,
                  { label: t('files.rename'), hint: 'F2', run: () => setEdit({ type: 'rename', root: menu.root, path: menu.entry!.path, dir: menu.entry!.dir }) },
                  { label: t('files.delete'), hint: 'Del', danger: true, run: () => setConfirmDel(menu.entry) },
                  null,
                  { label: t('files.copyPath'), run: () => void navigator.clipboard?.writeText(menu.entry!.path) },
                ]
              : []),
          ]}
        />
      ) : null}
    </div>
  );
}

/** A tree entry under the keyboard / menu. */
interface Entry {
  root: string;
  path: string;
  dir: boolean;
}

const parentOf = (p: string) => (p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : '');
const joinPath = (dir: string, name: string) => (dir ? `${dir}/${name.trim()}` : name.trim());

/** Inline name input in the tree (new file/folder, rename): Enter saves, Esc cancels. */
function NameInput({ depth, dir, initial, onCommit, onCancel }: { depth: number; dir: boolean; initial: string; onCommit: (name: string) => void; onCancel: () => void }) {
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    // select the name without the extension (like VS Code)
    const dot = initial.lastIndexOf('.');
    el.setSelectionRange(0, !dir && dot > 0 ? dot : initial.length);
  }, [dir, initial]);
  const finish = (save: boolean) => {
    if (done.current) return;
    done.current = true;
    const name = ref.current?.value.trim() ?? '';
    if (save && name && !name.includes('..')) onCommit(name);
    else onCancel();
  };
  return (
    <div style={{ paddingLeft: 8 + depth * 14 }} className="flex items-center gap-1.5 py-[2px] pr-2">
      <span className="w-3.5 shrink-0" />
      <FileIcon name={initial || (dir ? 'folder' : 'file')} dir={dir} />
      <input
        ref={ref}
        defaultValue={initial}
        placeholder={dir ? t('files.folderName') : t('files.fileName')}
        className="min-w-0 flex-1 border border-accent bg-bg px-1 py-[1px] text-[13px] outline-none"
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === 'Enter') finish(true);
          if (e.key === 'Escape') finish(false);
        }}
        onBlur={() => finish(true)}
      />
    </div>
  );
}

/** Right-click menu of the tree. */
function TreeMenu({ x, y, items, onClose }: { x: number; y: number; items: ({ label: string; hint?: string; danger?: boolean; run: () => void } | null)[]; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const off = (e: Event) => {
      if (e instanceof KeyboardEvent && e.key !== 'Escape') return;
      if (e.type === 'mousedown' && ref.current?.contains(e.target as Node)) return;
      onClose();
    };
    window.addEventListener('mousedown', off);
    window.addEventListener('keydown', off);
    window.addEventListener('blur', off);
    return () => {
      window.removeEventListener('mousedown', off);
      window.removeEventListener('keydown', off);
      window.removeEventListener('blur', off);
    };
  }, [onClose]);
  return (
    <div ref={ref} role="menu" className="fixed z-50 min-w-48 border border-line bg-surface py-1 text-[13px] shadow-lg" style={{ left: Math.min(x, window.innerWidth - 210), top: Math.min(y, window.innerHeight - 220) }}>
      {items.map((it, i) =>
        it ? (
          <button
            key={i}
            type="button"
            role="menuitem"
            onClick={() => {
              onClose();
              it.run();
            }}
            className={`flex w-full items-center gap-4 px-3 py-1.5 text-left hover:bg-sunken ${it.danger ? 'text-danger' : ''}`}
          >
            <span className="flex-1">{it.label}</span>
            {it.hint ? <span className="text-[11px] text-muted">{it.hint}</span> : null}
          </button>
        ) : (
          <div key={i} className="my-1 h-px bg-line" />
        ),
      )}
    </div>
  );
}

function ToolButton({ label, onClick, children, active }: { label: string; onClick: () => void; children: ReactNode; active?: boolean }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className={`inline-flex size-8 shrink-0 items-center justify-center rounded-lg hover:bg-sunken ${active ? 'text-accent' : 'text-muted hover:text-fg'}`}
    >
      {children}
    </button>
  );
}

/** Search across all project files (git ls-files) by substring match in the path. */
function FileSearch({ repos, query, enterTick, onSelect }: { repos: Repo[]; query: string; enterTick: number; onSelect: (s: Selection, pinned?: boolean) => void }) {
  const [files, setFiles] = useState<{ root: string; rootName: string; path: string }[] | null>(null);
  useEffect(() => {
    let alive = true;
    Promise.all(
      repos.map((r) =>
        api.projectFiles(r.id).then(
          (x) => x.files.map((path) => ({ root: r.id, rootName: r.name, path })),
          () => [],
        ),
      ),
    ).then((all) => alive && setFiles(all.flat()));
    return () => {
      alive = false;
    };
  }, [repos]);

  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const hits = (files ?? [])
    .filter((f) => words.every((w) => f.path.toLowerCase().includes(w)))
    .sort((a, b) => {
      const an = a.path.split('/').pop()!.toLowerCase().includes(words[0]) ? 0 : 1;
      const bn = b.path.split('/').pop()!.toLowerCase().includes(words[0]) ? 0 : 1;
      return an - bn || a.path.length - b.path.length;
    })
    .slice(0, 200);
  const first = hits[0];
  useEffect(() => {
    if (enterTick && first) onSelect({ repo: first.root, path: first.path }, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enterTick]);

  if (!files)
    return (
      <div className="flex justify-center py-6">
        <Spinner />
      </div>
    );
  if (!hits.length) return <div className="px-3 py-2 text-sm text-muted">{t('files.noMatches')}</div>;
  const multi = repos.length > 1;
  return (
    <ul>
      {hits.map((f) => {
        const i = f.path.lastIndexOf('/');
        return (
          <li key={`${f.root}:${f.path}`}>
            <button
              type="button"
              onClick={() => onSelect({ repo: f.root, path: f.path }, false)}
              onDoubleClick={() => onSelect({ repo: f.root, path: f.path }, true)}
              className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-[13.5px] hover:bg-sunken"
            >
              <FileIcon name={f.path.slice(f.path.lastIndexOf('/') + 1)} />
              <span className="shrink-0">{f.path.slice(i + 1)}</span>
              <span className="min-w-0 flex-1 truncate text-xs text-muted">
                {multi ? `${f.rootName}/` : ''}
                {i > 0 ? f.path.slice(0, i) : ''}
              </span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}
