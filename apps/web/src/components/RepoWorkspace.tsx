import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useSearchParams } from 'react-router';
import { FileText, Files, GitBranch, MessageSquare, Plus, Search, SquareTerminal, X, type LucideIcon } from 'lucide-react';
import type { GitStatus } from '@hubness/shared';
import { api } from '../lib/api';
import { useLocalStorage, useSessions, type SessionScope } from '../lib/hooks';
import { type Repo } from '../lib/repo';
import { AskContext, lastSessionKey, pushDraft, type AskApi, type AskSession } from '../lib/ask';
import * as L from '../lib/layout';
import { WorkbenchContext, type WorkbenchApi } from '../lib/workbench';
import { useFileChanges, useFileWatch } from '../lib/fileWatch';
import { t } from '../i18n';
import { Explorer } from './Explorer';
import { discardDraft, FileViewer, saveDraft } from './FileViewer';
import { choiceDialog, confirmDialog } from './Confirm';
import { FileIcon } from './FileIcon';
import { GitStatusLine } from './GitStatusLine';
import { useSideRight } from '../lib/editorPrefs';
import { SearchView } from './SearchView';
import { Empty, TopBar } from './ui';
import { usePhoneFullscreen } from './Layout';
import { JumpLabel } from './LeaderOverlay';
import { useKeyActions, type KeyAction } from '../lib/keys';
import { useDragging } from '../lib/dnd';
import { NewSessionView, SessionView } from './SessionView';
import { ptySize, ShellTab } from './project/shell';
import { StatusDot } from './ui';
import { ActivityButton, Title, ToolBadge, ToolBar, ToolLauncher } from './workspace/chrome';
import { GitTool, Readme } from './workspace/git';
import { ContextMenu, DropZone, GroupBody, TabStrip } from './workspace/tabs';

/**
 * git status of each repository: refreshed on changes on disk (the workbench's file stream is
 * opened here) and every 10 s as a fallback.
 */
export function useGitByRepo(repos: Repo[]) {
  const key = repos.map((r) => r.id).join(',');
  const [git, setGit] = useState<Record<string, GitStatus | null>>({});
  const refresh = useCallback(() => {
    for (const r of repos) api.gitStatus(r.id).then((g) => setGit((m) => ({ ...m, [r.id]: g })), () => setGit((m) => ({ ...m, [r.id]: null })));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  useEffect(() => {
    setGit({});
    refresh();
    const timer = setInterval(refresh, 10_000);
    return () => clearInterval(timer);
  }, [refresh]);
  useFileWatch(repos.map((r) => r.id));
  const soon = useRef<ReturnType<typeof setTimeout> | null>(null);
  useFileChanges(() => {
    soon.current ??= setTimeout(() => {
      soon.current = null;
      refresh();
    }, 300);
  });
  useEffect(() => () => void (soon.current && clearTimeout(soon.current)), []);
  return { git, refresh };
}

/** Project tool: opens as a tab in the editor area (Sessions, Run, Shell…). */
export interface Tool {
  id: string;
  label: string;
  icon: LucideIcon;
  /** Badge: a number (changes) or a dot (a run is in progress). */
  badge?: number | 'dot' | null;
  render: () => ReactNode;
}

type SideView = 'files' | 'search' | 'git';

/** Leader keys of side views and tools (tab numbers 1–9 jump inside the focused group). */
const SIDE_KEYS: Record<SideView, string> = { files: 'e', search: 'f', git: 'g' };
/** Icon colors of side views and tools: theme variables, so every theme keeps its palette. */
export const ICON_COLOR: Record<string, string> = {
  files: 'var(--syntax-entity, var(--color-accent))',
  search: 'var(--syntax-constant, var(--color-accent))',
  git: 'var(--color-accent)',
  sessions: 'var(--syntax-function, var(--color-accent))',
  run: 'var(--color-ok)',
  preview: 'var(--syntax-tag, var(--color-accent))',
  shell: 'var(--syntax-keyword, var(--color-accent))',
  secrets: 'var(--color-warn)',
  settings: 'var(--syntax-string, var(--color-muted))',
};

export const TOOL_KEYS: Record<string, string> = { sessions: 'c', run: 'r', preview: 'v', shell: 't', secrets: 'k', settings: 'o' };
type MobileView = SideView | 'editor';

const DESKTOP = '(min-width: 1024px)';

/** Re-render when the window width crosses the desktop/phone boundary. */
function useDesktop() {
  const [desk, setDesk] = useState(() => window.matchMedia(DESKTOP).matches);
  useEffect(() => {
    const m = window.matchMedia(DESKTOP);
    const fn = () => setDesk(m.matches);
    m.addEventListener('change', fn);
    return () => m.removeEventListener('change', fn);
  }, []);
  return desk;
}

const readLayout = (key: string): L.Layout | null => {
  try {
    return JSON.parse(localStorage.getItem(`hub.layout.${key}`) ?? 'null') as L.Layout | null;
  } catch {
    return null;
  }
};

/** URL ↔ the focused tab: ?file=&repo= for a file, ?t= for a tool, ?s= for a session (links and reloads keep working). */
const urlSig = (p: URLSearchParams) => `${p.get('t') ?? ''}|${p.get('repo') ?? ''}|${p.get('file') ?? ''}|${p.get('s') ?? ''}`;

let draftSeq = 0;
/** A new-session draft tab (several can be open at once). */
const draftRef = (): L.TabRef => ({ kind: 'session', id: `new:${Date.now().toString(36)}${(draftSeq++).toString(36)}` });

function fromUrl(l: L.Layout, p: URLSearchParams, repos: Repo[], tools: Set<string>): L.Layout {
  const file = p.get('file');
  const tool = p.get('t');
  const sid = p.get('s');
  // a session link wins (old links carry ?t=sessions&s=<id>)
  if (sid) return L.open(l, sid === 'new' ? draftRef() : { kind: 'session', id: sid });
  if (file) return L.open(l, { kind: 'file', repo: p.get('repo') ?? repos[0]?.id ?? '', path: file });
  // ?t=shell from old links: shells are tabs of their own now
  if (tool && tool !== 'shell' && tools.has(tool)) return L.open(l, { kind: 'tool', id: tool });
  return l;
}


/**
 * Project or group workbench (like VS Code / Neovim), without popups.
 *
 * Desktop: activity bar (Files, Search, Git) → side bar → editor groups
 * side by side. Every group has its own tab line; a tab is a file or a tool (Sessions,
 * Run, Preview, Shell…). Tabs are dragged between groups, onto the right edge to split,
 * reordered; right click — close others/right, split, move. Single click in the tree opens
 * a preview tab (italic), double click or an edit keeps it. Opened tabs stay mounted
 * (terminals and sessions are not recreated). Layout is remembered per project; the
 * focused tab is in the URL.
 *
 * Phone: one strip — Files / Search / Git, then open tabs and «+» for tools; each full screen.
 */
export function RepoWorkspace({
  storageKey,
  title,
  badge,
  subtitle,
  repos,
  git,
  refreshGit,
  tools,
  actions,
  empty,
  onOpenTools,
  sessions,
}: {
  /** Agent sessions of this project/group: each opens as its own tab. */
  sessions: { scope: SessionScope; root?: string; title: string };
  storageKey: string;
  title: string;
  badge?: string;
  subtitle?: ReactNode;
  repos: Repo[];
  /** git status by repository (the owner's useGitByRepo). */
  git: Record<string, GitStatus | null>;
  refreshGit?: () => void;
  tools: Tool[];
  /** Header buttons (Run). */
  actions?: ReactNode;
  /** Shown instead of the tree when there are no repositories (empty group). */
  empty?: ReactNode;
  /** Which tools have an open tab (the owner polls Run state only when needed). */
  onOpenTools?: (ids: string[]) => void;
}) {
  const desktop = useDesktop();
  const [params, setParams] = useSearchParams();
  // the owner rebuilds `tools` every render: key the set by ids so effects don't re-run
  const toolIdsKey = tools.map((x) => x.id).join(',');
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const toolIds = useMemo(() => new Set(tools.map((x) => x.id)), [toolIdsKey]);
  const toolById = useMemo(() => new Map(tools.map((x) => [x.id, x])), [tools]);

  const [layout, setLayoutRaw] = useState<L.Layout>(() => fromUrl(L.closeKey(L.sanitize(readLayout(storageKey), toolIds, { restore: true }), 't:shell'), params, repos, toolIds));
  const [side, setSide] = useLocalStorage<SideView>('hub.side.view', 'files');
  const [sideOpen, setSideOpen] = useLocalStorage('hub.side.open', true);
  const [sideWidth, setSideWidth] = useLocalStorage('hub.side.width', 280);
  const [mobileView, setMobileView] = useState<MobileView>(() => (params.get('file') || params.get('t') || params.get('s') ? 'editor' : 'files'));
  const [dirty, setDirty] = useState<Set<string>>(() => new Set());
  /**
   * Every layout change goes through here: a file tab with unsaved edits that would leave the
   * layout entirely (closed, not moved; its copy in another group keeps it open) asks first —
   * Save / Discard / Cancel. Cancel or a failed save keeps everything as it was.
   */
  const layoutRef = useRef(layout);
  layoutRef.current = layout;
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  const setLayout = useCallback((u: L.Layout | ((l: L.Layout) => L.Layout)) => {
    const fn = typeof u === 'function' ? u : () => u;
    const next = fn(layoutRef.current);
    const open = new Set(next.groups.flatMap((g) => g.tabs.map((x) => x.key)));
    const lost = new Map<string, { repo: string; path: string }>();
    for (const g of layoutRef.current.groups)
      for (const x of g.tabs) if (x.ref.kind === 'file' && dirtyRef.current.has(x.key) && !open.has(x.key)) lost.set(x.key, x.ref);
    if (!lost.size) return setLayoutRaw(fn);
    void (async () => {
      for (const ref of lost.values()) {
        const name = ref.path.split('/').pop() ?? ref.path;
        const choice = await choiceDialog({
          title: t('files.closeDirtyTitle'),
          message: t('files.closeDirty', { name }),
          choices: [
            { id: 'discard', label: t('files.dontSave'), variant: 'danger' },
            { id: 'save', label: t('common.save'), variant: 'primary' },
          ],
        });
        if (!choice) return;
        if (choice === 'discard') discardDraft(ref.repo, ref.path);
        else {
          const err = await saveDraft(ref.repo, ref.path);
          if (err) return void (await confirmDialog({ title: t('files.closeDirtyTitle'), message: err }));
        }
      }
      setLayoutRaw(fn);
    })();
  }, []);
  const [filesFocus, setFilesFocus] = useState(0);
  const [searchFocus, setSearchFocus] = useState(0);
  const dragging = useDragging();
  const [menu, setMenu] = useState<{ x: number; y: number; group: string; key: string } | null>(null);
  const [toolMenu, setToolMenu] = useState(false);

  // shell jobs with a tab, as of the last layout (see the effect below)
  const shellsOpen = useRef<Set<string> | null>(null);
  // the project changed (route reused): reload its layout
  const loadedFor = useRef(storageKey);
  useEffect(() => {
    if (loadedFor.current === storageKey) return;
    loadedFor.current = storageKey;
    // another project's layout: its shells are not "closed tabs"
    shellsOpen.current = null;
    setLayoutRaw(fromUrl(L.closeKey(L.sanitize(readLayout(storageKey), toolIds, { restore: true }), 't:shell'), params, repos, toolIds));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storageKey]);
  // tools may disappear (e.g. Run is only for normal projects)
  useEffect(() => setLayout((l) => L.sanitize(l, toolIds)), [toolIds]);
  useEffect(() => {
    try {
      localStorage.setItem(`hub.layout.${storageKey}`, JSON.stringify(layout));
    } catch {}
  }, [layout, storageKey]);

  // titles and statuses for session tabs
  const { sessions: sessionList } = useSessions(sessions.scope);

  const focused = L.focusedGroup(layout);
  const geo = useMemo(() => L.geometry(layout.root), [layout.root]);
  const current = L.activeTab(focused);

  // URL → layout (links, the Run button, "ask the agent"); layout → URL (the focused tab)
  const written = useRef(urlSig(params));
  const sig = urlSig(params);
  useEffect(() => {
    if (sig === written.current) return;
    written.current = sig;
    setLayout((l) => fromUrl(l, params, repos, toolIds));
    if (params.get('file') || params.get('t') || params.get('s')) setMobileView('editor');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sig]);
  useEffect(() => {
    const next = new URLSearchParams(params);
    const ref = current?.ref;
    for (const k of ['t', 'repo', 'file', 's']) next.delete(k);
    if (ref?.kind === 'file') {
      next.set('repo', ref.repo);
      next.set('file', ref.path);
    } else if (ref?.kind === 'tool') next.set('t', ref.id);
    else if (ref?.kind === 'session' && !ref.id.startsWith('new:')) next.set('s', ref.id);
    const s = urlSig(next);
    if (s === sig) return;
    written.current = s;
    setParams(next, { replace: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current?.key]);

  const openToolsKey = layout.groups.flatMap((g) => g.tabs.filter((x) => x.ref.kind === 'tool').map((x) => (x.ref as { id: string }).id)).sort().join(',');
  useEffect(() => onOpenTools?.(openToolsKey ? openToolsKey.split(',') : []), [openToolsKey, onOpenTools]);

  const api_: WorkbenchApi = useMemo(
    () => ({
      openFile: (repo, path, opts) => {
        setLayout((l) => L.open(l, { kind: 'file', repo, path }, opts));
        setMobileView('editor');
      },
      openTool: (id) => {
        setMobileView('editor');
        // Shell: every click is a new shell in its own tab
        if (id === 'shell') {
          void api.shell(sessions.scope, { ...ptySize(), fresh: true }).then(
            (job) => setLayout((l) => L.open(l, { kind: 'shell', id: job.id, title: job.title })),
            () => {},
          );
          return;
        }
        setLayout((l) => L.open(l, { kind: 'tool', id }));
      },
      openSession: (id, opts) => {
        if (id !== 'new') {
          try {
            localStorage.setItem(lastSessionKey(storageKey), JSON.stringify(id));
          } catch {}
        }
        setLayout((l) => {
          // already open in a group (e.g. next to the file): show that tab instead of a copy here
          if (id !== 'new' && !opts?.group && !opts?.replace) {
            const key = L.tabKey({ kind: 'session', id });
            const g = l.groups.find((x) => x.active === key) ?? l.groups.find((x) => x.tabs.some((tab) => tab.key === key));
            if (g) return L.activate(l, g.id, key);
          }
          return L.open(l, id === 'new' ? draftRef() : { kind: 'session', id }, opts);
        });
        setMobileView('editor');
      },
      renamed: (repo, from, to) => setLayout((l) => L.renamePath(l, repo, from, to)),
      deleted: (repo, path) => setLayout((l) => L.closePath(l, repo, path)),
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [storageKey],
  );

  // running shells without a tab (opened before shells became tabs, or on another device)
  // get one, so none keeps running out of reach
  useEffect(() => {
    let alive = true;
    api.shells(sessions.scope).then(
      (list) => {
        if (!alive || !list.length) return;
        setLayout((l) => {
          const have = L.shellIds(l);
          return list.filter((j) => !have.has(j.id)).reduce((acc, j) => L.open(acc, { kind: 'shell', id: j.id, title: j.title }), l);
        });
      },
      () => {},
    );
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storageKey]);

  // a shell whose last tab was closed is stopped (copies in other groups keep it alive)
  useEffect(() => {
    const now = L.shellIds(layout);
    const before = shellsOpen.current;
    shellsOpen.current = now;
    if (!before) return;
    for (const id of before) if (!now.has(id)) void api.jobKill(id).catch(() => {});
  }, [layout]);

  // "ask the agent" about a selection: a draft in a chosen session (or a new one)
  const askSessions = useMemo<AskSession[]>(() => {
    const title = (id: string) => sessionList?.find((x) => x.id === id)?.title || t('psessions.new');
    const ids: string[] = [];
    const add = (id: string | undefined) => id && !id.startsWith('new:') && id !== 'new' && !ids.includes(id) && ids.push(id);
    // the session in view first, then the ones shown in other groups, then the rest of the tabs
    const inView = (g: L.Group) => (L.activeTab(g)?.ref.kind === 'session' ? (L.activeTab(g)!.ref as { id: string }).id : undefined);
    add(inView(L.focusedGroup(layout)));
    for (const g of layout.groups) add(inView(g));
    for (const g of layout.groups) for (const tab of g.tabs) if (tab.ref.kind === 'session') add(tab.ref.id);
    if (!ids.length) {
      let last: string | null = null;
      try {
        last = JSON.parse(localStorage.getItem(lastSessionKey(storageKey)) ?? 'null');
      } catch {}
      if (last && (!sessionList || sessionList.some((x) => x.id === last))) add(last);
    }
    return ids.map((id) => ({ id, title: title(id) }));
  }, [layout, sessionList, storageKey]);
  const ask: AskApi = {
    sessions: askSessions,
    ask: (text, sid) => {
      if (sid === 'new') {
        // drafts of new sessions share one key, like the Sessions tool always did
        pushDraft(`${storageKey}:new`, text);
        api_.openSession('new');
      } else {
        pushDraft(`${storageKey}:${sid}`, text);
        api_.openSession(sid);
      }
    },
  };

  const showSide = (v: SideView) => {
    if (!desktop) return setMobileView(v);
    if (sideOpen && side === v) return setSideOpen(false);
    setSide(v);
    setSideOpen(true);
  };

  // keyboard (desktop): not inside a terminal — it needs its own Ctrl keys
  useEffect(() => {
    if (!desktop) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as Element | null)?.closest?.('.xterm')) return;
      const mod = e.ctrlKey || e.metaKey;
      const hit = (fn: () => void) => {
        e.preventDefault();
        fn();
      };
      if (mod && !e.shiftKey && e.code === 'KeyP')
        hit(() => {
          setSide('files');
          setSideOpen(true);
          setFilesFocus((n) => n + 1);
        });
      else if (mod && e.shiftKey && e.code === 'KeyF')
        hit(() => {
          setSide('search');
          setSideOpen(true);
          setSearchFocus((n) => n + 1);
        });
      else if (mod && e.shiftKey && e.code === 'KeyE') hit(() => (setSide('files'), setSideOpen(true)));
      else if (mod && e.shiftKey && e.code === 'KeyG') hit(() => (setSide('git'), setSideOpen(true)));
      else if (mod && !e.shiftKey && e.code === 'KeyB') hit(() => setSideOpen(!sideOpen));
      else if (mod && e.code === 'Backslash') hit(() => setLayout((l) => L.split(l, l.focused, e.shiftKey ? 'down' : 'right')));
      else if (e.altKey && e.code === 'KeyW')
        hit(() =>
          setLayout((l) => {
            const g = L.focusedGroup(l);
            return g.active ? L.close(l, g.id, g.active) : l;
          }),
        );
      else if (e.altKey && /^Digit[1-9]$/.test(e.code))
        hit(() =>
          setLayout((l) => {
            const id = L.groupOrder(l)[Number(e.code.slice(5)) - 1];
            return id ? L.focus(l, id) : l;
          }),
        );
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [desktop, sideOpen, setSide, setSideOpen]);

  // side bar width — drag its right edge
  const sideRight = useSideRight();
  const sideDrag = useRef<{ x: number; w: number; dir: 1 | -1 } | null>(null);
  // divider between groups — drag (works in both directions of the split tree)
  const mainRef = useRef<HTMLDivElement>(null);
  const groupDrag = useRef<{ div: L.Divider; at: number } | null>(null);
  useEffect(() => {
    const move = (e: PointerEvent) => {
      if (sideDrag.current) {
        e.preventDefault();
        setSideWidth(Math.round(Math.max(200, Math.min(560, sideDrag.current.w + (e.clientX - sideDrag.current.x) * sideDrag.current.dir))));
      } else if (groupDrag.current && mainRef.current) {
        e.preventDefault();
        const { div } = groupDrag.current;
        const row = div.dir === 'row';
        const pos = row ? e.clientX : e.clientY;
        // pixels → share of that split
        const span = (row ? mainRef.current.clientWidth * div.parent.w : mainRef.current.clientHeight * div.parent.h) || 1;
        const d = (pos - groupDrag.current.at) / span;
        groupDrag.current.at = pos;
        setLayout((l) => L.resize(l, div.path, div.index, d));
      }
    };
    const up = () => {
      sideDrag.current = null;
      groupDrag.current = null;
      document.body.style.userSelect = '';
      document.body.style.cursor = '';
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    return () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
  }, [setSideWidth]);

  const changes = repos.reduce((n, r) => n + (git[r.id]?.files.length ?? 0), 0);
  const multi = repos.length > 1;
  const selected = current?.ref.kind === 'file' ? { repo: current.ref.repo, path: current.ref.path } : null;
  const setDirtyKey = useCallback(
    (key: string, d: boolean) =>
      setDirty((s) => {
        if (s.has(key) === d) return s;
        const n = new Set(s);
        if (d) n.add(key);
        else n.delete(key);
        return n;
      }),
    [],
  );

  const sideViews: { id: SideView; label: string; icon: LucideIcon; badge?: number }[] = [
    { id: 'files', label: t('side.files'), icon: Files },
    { id: 'search', label: t('side.search'), icon: Search },
    ...(repos.length ? [{ id: 'git' as const, label: t('side.git'), icon: GitBranch, badge: changes }] : []),
  ];

  const statusLine = repos.length ? (
    <GitStatusLine
      repos={repos}
      git={git}
      onOpen={() => {
        if (!desktop) return setMobileView('git');
        setSide('git');
        setSideOpen(true);
      }}
    />
  ) : null;

  const renderSide = (v: SideView) => {
    if (v === 'files')
      return repos.length ? (
        <Explorer
          storageKey={storageKey}
          repos={repos}
          gitByRoot={git}
          selected={selected}
          focusNonce={filesFocus}
          onSelect={(s, pinned) => api_.openFile(s.repo, s.path, { preview: !pinned })}
          reloadKey={0}
        />
      ) : (
        (empty ?? <Empty title={t('groups.noRepos')} />)
      );
    if (v === 'search') return <SearchView repos={repos} focusNonce={searchFocus} />;
    return <GitTool repos={repos} git={git} refresh={refreshGit ?? (() => {})} />;
  };

  const renderTab = (tab: L.Tab, groupId: string) => {
    if (tab.ref.kind === 'tool') return toolById.get(tab.ref.id)?.render() ?? null;
    if (tab.ref.kind === 'shell') {
      const key = tab.key;
      return <ShellTab jobId={tab.ref.id} title={tab.ref.title} onExit={() => setLayout((l) => L.closeKey(l, key))} />;
    }
    if (tab.ref.kind === 'session') {
      const ref = tab.ref;
      // back: this tab turns into the Sessions list again
      const toList = () => setLayout((l) => L.open(l, { kind: 'tool', id: 'sessions' }, { group: groupId, replace: tab.key }));
      const base = 'projectId' in sessions.scope ? `/projects/${sessions.scope.projectId}/s` : `/groups/${sessions.scope.groupId}/s`;
      return ref.id.startsWith('new:') ? (
        <NewSessionView
          scope={sessions.scope}
          backTo=""
          basePath={base}
          title={t('session.newIn', { project: sessions.title })}
          onBack={toList}
          onCreated={(id) => {
            try {
              localStorage.setItem(lastSessionKey(storageKey), JSON.stringify(id));
            } catch {}
            setLayout((l) => L.retarget(l, ref, { kind: 'session', id }));
          }}
          draftKey={`${storageKey}:new`}
        />
      ) : (
        <SessionView key={ref.id} sessionId={ref.id} backTo="" root={sessions.root} onBack={toList} draftKey={`${storageKey}:${ref.id}`} />
      );
    }
    const { repo, path } = tab.ref;
    return (
      <FileViewer
        projectId={repo}
        rootName={multi ? repos.find((r) => r.id === repo)?.name : undefined}
        path={path}
        git={git[repo] ?? null}
        line={tab.line}
        nonce={tab.nonce}
        initialMode={tab.mode}
        onDirty={(d) => setDirtyKey(tab.key, d)}
        onPin={() => setLayout((l) => L.pin(l, groupId, tab.key))}
        onBack={desktop ? undefined : () => setMobileView('files')}
      />
    );
  };

  const welcome = repos[0] ? (
    <Readme repo={repos[0]} multi={multi} />
  ) : (
    <Empty icon={<FileText className="size-8" />} title={t('files.pick')} hint={t('files.pickHint')} />
  );

  const tabLabel = (tab: L.Tab, group: L.Group) => {
    if (tab.ref.kind === 'shell') return { name: tab.ref.title, hint: '', icon: <SquareTerminal className="size-3.5 shrink-0" style={{ color: ICON_COLOR.shell }} />, badge: null };
    if (tab.ref.kind === 'session') {
      const ref = tab.ref;
      const s = sessionList?.find((x) => x.id === ref.id);
      const name = ref.id.startsWith('new:') ? t('psessions.newSession') : (s?.title || t('psessions.new'));
      return { name, hint: '', icon: s ? <StatusDot status={s.status} /> : <MessageSquare className="size-3.5 shrink-0" />, badge: null };
    }
    if (tab.ref.kind === 'tool') {
      const tool = toolById.get(tab.ref.id);
      return { name: tool?.label ?? tab.ref.id, hint: '', icon: tool ? <tool.icon className="size-3.5 shrink-0" /> : null, badge: tool?.badge };
    }
    const path = tab.ref.path;
    const name = path.slice(path.lastIndexOf('/') + 1);
    // same name twice in the group — show the folder (like VS Code)
    const twin = group.tabs.some((o) => o !== tab && o.ref.kind === 'file' && o.ref.path.slice(o.ref.path.lastIndexOf('/') + 1) === name);
    const dir = path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '';
    const repoName = multi ? repos.find((r) => r.id === (tab.ref as { repo: string }).repo)?.name : '';
    return { name, hint: twin ? [repoName, dir].filter(Boolean).join('/') : '', icon: <FileIcon name={name} className="size-4 shrink-0" />, badge: null };
  };

  // leader-key actions (see lib/keys.ts)
  const sec = { tabs: t('keys.tabs'), views: t('keys.views'), tools: t('keys.tools') };
  const step = (d: number) =>
    setLayout((l) => {
      const g = L.focusedGroup(l);
      const i = g.tabs.findIndex((x) => x.key === g.active);
      const next = g.tabs[(i + d + g.tabs.length) % Math.max(1, g.tabs.length)];
      return next ? L.activate(l, g.id, next.key) : l;
    });
  const stepGroup = (d: number) =>
    setLayout((l) => {
      const order = L.groupOrder(l);
      const id = order[order.indexOf(l.focused) + d];
      return id ? L.focus(l, id) : l;
    });
  const keyActions: KeyAction[] = [
    ...focused.tabs.slice(0, 9).map((tab, i) => ({
      key: String(i + 1),
      label: tabLabel(tab, focused).name,
      section: sec.tabs,
      run: () => {
        setLayout((l) => L.activate(l, focused.id, tab.key));
        setMobileView('editor');
      },
    })),
    { key: 'h', label: t('keys.prevTab'), section: sec.tabs, run: () => step(-1) },
    { key: 'l', label: t('keys.nextTab'), section: sec.tabs, run: () => step(1) },
    { key: 'w', label: t('tabs.close'), section: sec.tabs, run: () => setLayout((l) => (L.focusedGroup(l).active ? L.close(l, l.focused, L.focusedGroup(l).active!) : l)) },
    ...(desktop
      ? [
          { key: 's', label: t('tabs.split'), section: sec.tabs, run: () => setLayout((l) => L.split(l, l.focused, 'right')) },
          { key: 'd', label: t('tabs.splitDown'), section: sec.tabs, run: () => setLayout((l) => L.split(l, l.focused, 'down')) },
          { key: '[', label: t('keys.prevGroup'), section: sec.tabs, run: () => stepGroup(-1) },
          { key: ']', label: t('keys.nextGroup'), section: sec.tabs, run: () => stepGroup(1) },
          { key: 'b', label: t('keys.sidebar'), section: sec.views, run: () => setSideOpen(!sideOpen) },
        ]
      : []),
    ...sideViews.map((v) => ({ key: SIDE_KEYS[v.id], label: v.label, section: sec.views, run: () => (desktop ? (setSide(v.id), setSideOpen(true)) : setMobileView(v.id)) })),
    {
      key: 'p',
      label: t('keys.goToFile'),
      section: sec.views,
      run: () => {
        if (desktop) {
          setSide('files');
          setSideOpen(true);
        } else setMobileView('files');
        setFilesFocus((n) => n + 1);
      },
    },
    ...tools.filter((x) => TOOL_KEYS[x.id]).map((x) => ({ key: TOOL_KEYS[x.id]!, label: x.label, section: sec.tools, run: () => api_.openTool(x.id) })),
  ];
  useKeyActions('workbench', keyActions);

  // phone: an open session takes the whole screen like a chat (its own header with «back»):
  // no project bar, tab strip or bottom navigation around it
  const sessionFull = !desktop && mobileView === 'editor' && focused.tabs.find((x) => x.key === focused.active)?.ref.kind === 'session';
  usePhoneFullscreen(sessionFull);

  const ctx = (
    <ContextMenu
      menu={menu}
      layout={layout}
      onClose={() => setMenu(null)}
      apply={(fn) => {
        setLayout(fn);
        setMenu(null);
      }}
    />
  );

  // ---------- phone ----------
  if (!desktop) {
    const all = layout.groups.flatMap((g) => g.tabs.map((tab) => ({ tab, g })));
    return (
      <WorkbenchContext.Provider value={api_}>
        <AskContext.Provider value={ask}>
          <div className="flex min-h-0 flex-1 flex-col">
            {sessionFull ? null : <TopBar title={<Title title={title} badge={badge} />} subtitle={subtitle} back="/projects" actions={actions} />}
            <nav className={`shrink-0 items-stretch border-b border-line bg-surface ${sessionFull ? 'hidden' : 'flex'}`}>
              {sideViews.map((v) => (
                <button
                  key={v.id}
                  type="button"
                  title={v.label}
                  aria-label={v.label}
                  onClick={() => setMobileView(v.id)}
                  className={`relative inline-flex w-10 shrink-0 items-center justify-center border-b-2 pointer-coarse:w-12 ${mobileView === v.id ? 'border-accent text-accent' : 'border-transparent text-muted'}`}
                >
                  <v.icon className="size-4 pointer-coarse:size-5" style={{ color: ICON_COLOR[v.id] }} />
                  {v.badge ? <span className="absolute top-1 right-0.5 text-[10px] text-warn">{v.badge}</span> : null}
                </button>
              ))}
              <span className="my-2 w-px shrink-0 bg-line" />
              <div className="flex min-w-0 flex-1 overflow-x-auto [scrollbar-width:none]">
                {all.map(({ tab, g }) => {
                  const lb = tabLabel(tab, g);
                  const on = mobileView === 'editor' && g.id === focused.id && g.active === tab.key;
                  const idx = g.id === focused.id ? focused.tabs.indexOf(tab) : -1;
                  return (
                    <div key={`${g.id}:${tab.key}`} className={`relative flex shrink-0 items-center border-b-2 ${on ? 'border-accent text-fg' : 'border-transparent text-muted'}`}>
                      <JumpLabel k={idx >= 0 && idx < 9 ? String(idx + 1) : undefined} />
                      <button
                        type="button"
                        onClick={() => {
                          setLayout((l) => L.activate(l, g.id, tab.key));
                          setMobileView('editor');
                        }}
                        className={`flex max-w-[60vw] items-center gap-1.5 py-2 pl-2.5 text-[13px] whitespace-nowrap pointer-coarse:py-3.5 ${tab.preview ? 'italic' : ''}`}
                      >
                        {lb.icon}
                        <span className="truncate">{lb.name}</span>
                        <ToolBadge b={lb.badge} />
                        {dirty.has(tab.key) ? <span className="size-1.5 rounded-full bg-fg" /> : null}
                      </button>
                      <button type="button" aria-label={t('tabs.close')} onClick={() => setLayout((l) => L.close(l, g.id, tab.key))} className="tap inline-flex items-center justify-center px-1.5 py-2 text-muted">
                        <X className="size-3.5 pointer-coarse:size-4" />
                      </button>
                    </div>
                  );
                })}
              </div>
              <div className="relative shrink-0">
                <button type="button" aria-label={t('tabs.openTool')} title={t('tabs.openTool')} onClick={() => setToolMenu(!toolMenu)} className="inline-flex h-full w-10 items-center justify-center text-muted pointer-coarse:w-12">
                  <Plus className="size-4 pointer-coarse:size-5" />
                </button>
                {toolMenu ? (
                  <>
                    <div className="fixed inset-0 z-30" onClick={() => setToolMenu(false)} />
                    <div className="absolute top-full right-0 z-40 min-w-44 border border-line bg-surface py-1 shadow-lg">
                      {tools.map((x) => (
                        <button
                          key={x.id}
                          type="button"
                          onClick={() => {
                            setToolMenu(false);
                            api_.openTool(x.id);
                          }}
                          className="flex w-full items-center gap-2 px-3 py-2 text-left text-sm hover:bg-sunken pointer-coarse:py-3.5 pointer-coarse:text-[15px]"
                        >
                          <x.icon className="size-4 text-muted" /> {x.label} <ToolBadge b={x.badge} />
                        </button>
                      ))}
                    </div>
                  </>
                ) : null}
              </div>
            </nav>
            {sideViews.map((v) => (
              <div key={v.id} className={`min-h-0 flex-1 flex-col bg-surface ${mobileView === v.id ? 'flex' : 'hidden'}`}>
                {renderSide(v.id)}
              </div>
            ))}
            {mobileView !== 'editor' ? <div className="bg-surface">{statusLine}</div> : null}
            <div className={`min-h-0 flex-1 flex-col ${mobileView === 'editor' ? 'flex' : 'hidden'}`}>
              {focused.tabs.length ? (
                <GroupBody key={focused.id} group={focused} renderTab={renderTab} />
              ) : (
                <ToolLauncher tools={tools} onOpen={api_.openTool}>
                  {welcome}
                </ToolLauncher>
              )}
            </div>
          </div>
        </AskContext.Provider>
      </WorkbenchContext.Provider>
    );
  }

  // ---------- desktop ----------
  return (
    <WorkbenchContext.Provider value={api_}>
      <AskContext.Provider value={ask}>
        <div className="flex min-h-0 flex-1 flex-col">
          <TopBar
            title={<Title title={title} badge={badge} />}
            subtitle={subtitle}
            back="/projects"
          />
          <div className={`flex min-h-0 flex-1 ${sideRight ? 'flex-row-reverse' : ''}`}>
            {/* side bar (left or right, Settings → Appearance): the views row on top (tools live in the editor tab bar), views stay mounted */}
            {sideOpen ? (
              <aside className={`relative flex min-h-0 shrink-0 flex-col border-line bg-surface ${sideRight ? 'border-l' : 'border-r'}`} style={{ width: sideWidth }}>
                <div className="flex h-9 shrink-0 items-stretch border-b border-line">
                  {sideViews.map((v) => (
                    <ActivityButton key={v.id} label={v.label} text={sideWidth >= 340 ? (v.id === 'git' ? t('side.gitShort') : v.label) : undefined} active={side === v.id} onClick={() => showSide(v.id)} badge={v.badge} jump={SIDE_KEYS[v.id]}>
                      <v.icon className="size-4" style={{ color: ICON_COLOR[v.id] }} />
                    </ActivityButton>
                  ))}
                </div>
                {sideViews.map((v) => (
                  <div key={v.id} className={`min-h-0 flex-1 flex-col ${side === v.id ? 'flex' : 'hidden'}`}>
                    {renderSide(v.id)}
                  </div>
                ))}
                {statusLine}
                <div
                  onPointerDown={(e) => {
                    sideDrag.current = { x: e.clientX, w: sideWidth, dir: sideRight ? -1 : 1 };
                    document.body.style.userSelect = 'none';
                    document.body.style.cursor = 'col-resize';
                  }}
                  className={`absolute inset-y-0 z-10 w-2 cursor-col-resize hover:bg-accent/30 ${sideRight ? '-left-1' : '-right-1'}`}
                  title={t('ws.resize')}
                />
              </aside>
            ) : null}

            {/* editor groups: absolutely placed by the split tree, so changing the tree never
                remounts a group (terminals, editors and sessions keep their state) */}
            <div className="flex min-h-0 min-w-0 flex-1 flex-col">
              {/* tools are always one click away, whatever tabs are open */}
              <ToolBar tools={tools} layout={layout} onOpen={api_.openTool} />
              <div ref={mainRef} className="relative min-h-0 min-w-0 flex-1">
                {layout.groups.map((g) => {
                  const r = geo.rects.get(g.id) ?? { x: 0, y: 0, w: 1, h: 1 };
                  return (
                    <section
                      key={g.id}
                      className="absolute flex min-h-0 min-w-0 flex-col"
                      style={{ left: `${r.x * 100}%`, top: `${r.y * 100}%`, width: `${r.w * 100}%`, height: `${r.h * 100}%` }}
                      onPointerDownCapture={() => g.id !== layout.focused && setLayout((l) => L.focus(l, g.id))}
                    >
                      {g.tabs.length || layout.groups.length > 1 ? (
                        <TabStrip
                          group={g}
                          focused={g.id === layout.focused}
                          canSplit={layout.groups.length < L.MAX_GROUPS}
                          multiGroup={layout.groups.length > 1}
                          label={(tab) => tabLabel(tab, g)}
                          dirty={dirty}
                          setLayout={setLayout}
                          onMenu={(e, key) => {
                            e.preventDefault();
                            setMenu({ x: e.clientX, y: e.clientY, group: g.id, key });
                          }}
                        />
                      ) : null}
                      <div className="relative flex min-h-0 flex-1 flex-col">
                        {g.tabs.length ? (
                          <GroupBody group={g} focused={g.id === layout.focused} renderTab={renderTab} />
                        ) : (
                          <div className="flex min-h-0 flex-1 flex-col overflow-auto">{welcome}</div>
                        )}
                        {dragging ? <DropZone group={g} canSplit={layout.groups.length < L.MAX_GROUPS} setLayout={setLayout} /> : null}
                      </div>
                    </section>
                  );
                })}
                {geo.dividers.map((d) => {
                  const row = d.dir === 'row';
                  return (
                    <div
                      key={`${d.path.join('.')}:${d.index}`}
                      onPointerDown={(e) => {
                        groupDrag.current = { div: d, at: row ? e.clientX : e.clientY };
                        document.body.style.userSelect = 'none';
                        document.body.style.cursor = row ? 'col-resize' : 'row-resize';
                      }}
                      className={`absolute z-10 bg-line hover:bg-accent/60 after:absolute ${row ? 'w-px cursor-col-resize after:inset-y-0 after:-right-1 after:-left-1' : 'h-px cursor-row-resize after:inset-x-0 after:-top-1 after:-bottom-1'}`}
                      style={row ? { left: `${d.rect.x * 100}%`, top: `${d.rect.y * 100}%`, height: `${d.rect.h * 100}%` } : { left: `${d.rect.x * 100}%`, top: `${d.rect.y * 100}%`, width: `${d.rect.w * 100}%` }}
                    />
                  );
                })}
              </div>
            </div>
          </div>
          {ctx}
        </div>
      </AskContext.Provider>
    </WorkbenchContext.Provider>
  );
}
