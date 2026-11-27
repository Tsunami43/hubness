import { createContext, Suspense, useCallback, useContext, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react';
import { Link, NavLink, Outlet, useLocation, useNavigate } from 'react-router';
import { Activity, Archive, ArchiveRestore, Boxes, Cpu, FolderGit2, House, LogOut, Menu, MessageSquare, PanelRightClose, Settings, SquarePen, Trash2, X, type LucideIcon } from 'lucide-react';
import { useAccess, useAuth } from './AuthGate';
import type { Session } from '@hubness/shared';
import { api } from '../lib/api';
import { timeAgo } from '../lib/format';
import { useArchivedSessions, useLocalStorage, useRecentSessions, useSessionSync } from '../lib/hooks';
import { useAgents } from '../lib/agents';
import { t } from '../i18n';
import { Button, ErrorLine, PageSpinner, StatusDot } from './ui';
import { confirmDialog } from './Confirm';
import { matchesQuery, SessionSearch } from './SessionSearch';
import { AgentIcon } from './AgentIcon';
import { JumpLabel, LeaderOverlay } from './LeaderOverlay';
import { useKeyActions } from '../lib/keys';
import { slideOut, useSwipe } from '../lib/swipe';

// key: leader shortcut (uppercase = with Shift, so it never clashes with screen keys)
const NAV = [
  { to: '/', label: 'nav.new', icon: SquarePen, key: 'N' },
  { to: '/projects', label: 'nav.projects', icon: FolderGit2, key: 'P' },
  { to: '/system', label: 'nav.system', icon: Activity, key: 'S' },
  { to: '/harness', label: 'nav.harness', icon: Cpu, key: 'A' },
  { to: '/settings', label: 'nav.settings', icon: Settings, key: ',' },
] as const;

const isDesktop = () => window.matchMedia('(min-width: 1024px)').matches;

/** Right panel with all sessions: a column on the right on desktop, slides over the content on phone. */
const NavContext = createContext<{ historyOpen: boolean; toggleHistory: () => void; openHistory: () => void; setFullscreen: (on: boolean) => void }>({
  historyOpen: false,
  toggleHistory: () => {},
  openHistory: () => {},
  setFullscreen: () => {},
});
export const useNav = () => useContext(NavContext);

/** A screen that takes the whole phone screen while `on` (a session inside a project): hides the bottom navigation. */
export function usePhoneFullscreen(on: boolean) {
  const { setFullscreen } = useNav();
  useEffect(() => {
    if (!on) return;
    setFullscreen(true);
    return () => setFullscreen(false);
  }, [on, setFullscreen]);
}

/** An open session takes the whole screen on phone, without the bottom navigation. */
function isFullscreen(path: string, search: string) {
  return /^\/chats\/[^/]+$/.test(path) || (/^\/(projects|groups)\/[^/]+$/.test(path) && /[?&]s=/.test(search) && /[?&]t=sessions/.test(search));
}

/** A project/group session opens in the workspace's Sessions tab. */
export const sessionPath = (s: Pick<Session, 'id' | 'projectId' | 'groupId'>) =>
  s.projectId ? `/projects/${s.projectId}?t=sessions&s=${s.id}` : s.groupId ? `/groups/${s.groupId}?t=sessions&s=${s.id}` : `/chats/${s.id}`;

/** Where a session lives: shown as an icon + color + name, so projects, groups and chats differ at a glance. */
interface ScopeTag {
  name: string;
  icon: LucideIcon;
  /** Theme token class. */
  color: string;
}
const CHAT_TAG = (): ScopeTag => ({ name: t('history.chat'), icon: MessageSquare, color: 'text-muted' });

/** Scope tags of projects and groups (refetched when an unknown id shows up). */
function useScopeTags(sessions: Session[] | null | undefined) {
  const [tags, setTags] = useState<Map<string, ScopeTag>>(new Map());
  const unknown = sessions?.some((s) => (s.projectId && !tags.has(s.projectId)) || (s.groupId && !tags.has(s.groupId)));
  useEffect(() => {
    Promise.all([api.projects(), api.groups()]).then(
      ([ps, gs]) =>
        setTags(
          new Map([
            ...ps.map((p) => [p.id, p.kind === 'system' ? { name: p.name, icon: House, color: 'text-warn' } : { name: p.name, icon: FolderGit2, color: 'text-accent' }] as const),
            ...gs.map((g) => [g.id, { name: g.name, icon: Boxes, color: 'text-ok' }] as const),
          ]),
        ),
      () => {},
    );
  }, [unknown]);
  return (s: Session): ScopeTag => {
    const id = s.projectId ?? s.groupId;
    return (id ? tags.get(id) : null) ?? (id ? { name: '…', icon: s.groupId ? Boxes : FolderGit2, color: 'text-muted' } : CHAT_TAG());
  };
}

/** Small row action: always visible on touch screens, on hover/focus with a mouse. */
const rowAction =
  'tap inline-flex size-7 shrink-0 items-center justify-center text-muted opacity-0 group-hover:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100 hover:bg-surface hover:text-fg';

/**
 * One session row: icon with status, title, scope · time, and row actions on the right.
 * `onSwipe` (touch): swiping the row to the left commits it, like archiving in a mail app.
 */
function SessionRow({
  s,
  scope,
  when,
  active,
  onPick,
  actions,
  onSwipe,
}: {
  s: Session;
  scope: ScopeTag;
  when: string;
  active: boolean;
  onPick?: () => void;
  actions: ReactNode;
  onSwipe?: { label: string; icon: LucideIcon; run: (reset: () => void) => void };
}) {
  const row = useRef<HTMLDivElement>(null);
  const hint = useRef<HTMLSpanElement>(null);
  const swipe = useSwipe({
    dir: -1,
    target: () => row.current,
    threshold: 0.4,
    onProgress: (_, commit) => hint.current?.classList.toggle('text-accent', commit),
    onCommit: (reset) => onSwipe?.run(reset),
  });
  const body = (
    <div ref={row} className={`group flex items-start border-l-2 ${active ? 'border-accent bg-sunken' : 'border-transparent bg-surface hover:bg-sunken'}`}>
      <Link to={sessionPath(s)} onClick={onPick} className="flex min-w-0 flex-1 items-start gap-2.5 py-1.5 pl-2.5 pointer-coarse:py-3">
        <span className="relative mt-0.5">
          <AgentIcon kind={s.agent.kind} />
          <span className="absolute -right-1 -bottom-1 flex rounded-full bg-surface p-[2px]">
            <StatusDot status={s.status} />
          </span>
        </span>
        <span className="min-w-0 flex-1">
          <span className={`block truncate text-sm ${active ? 'font-medium text-fg' : 'text-fg/85'}`}>{s.title}</span>
          <span className="flex min-w-0 items-center gap-1 text-xs text-muted">
            <scope.icon className={`size-3 shrink-0 ${scope.color}`} />
            <span className={`truncate ${scope.color}`}>{scope.name}</span>
            <span className="shrink-0">· {timeAgo(when)}</span>
          </span>
        </span>
      </Link>
      <span className="flex shrink-0 items-center gap-0.5 py-1.5 pr-1.5 pointer-coarse:py-0.5">{actions}</span>
    </div>
  );
  if (!onSwipe) return <li>{body}</li>;
  return (
    <li className="relative overflow-hidden" {...swipe}>
      {/* revealed under the row while it slides away */}
      <span ref={hint} aria-hidden className="absolute inset-y-0 right-0 flex items-center gap-2 bg-sunken px-5 text-sm text-muted transition-colors">
        <onSwipe.icon className="size-4" /> {onSwipe.label}
      </span>
      <div className="relative">{body}</div>
    </li>
  );
}

const isOpen = (s: Session, pathname: string, search: string) => {
  const href = sessionPath(s);
  return pathname + search === href || (href.includes('?') && pathname === href.split('?')[0] && search.includes(`s=${s.id}`));
};

/** List of past sessions: chats and project sessions, newest first. */
export function History({ onPick, query = '' }: { onPick?: () => void; query?: string }) {
  const { sessions: all, loading } = useRecentSessions();
  const scopeTag = useScopeTags(all);
  const { pathname, search } = useLocation();
  const [error, setError] = useState<string | null>(null);
  const sessions = all?.filter((s) => matchesQuery(query, s.title, scopeTag(s).name));

  if (loading && !all) return null;
  if (!all?.length) return <div className="px-3 py-2 text-sm text-muted">{t('history.empty')}</div>;
  if (!sessions?.length) return <div className="px-3 py-2 text-sm text-muted">{t('sessions.nothingFound')}</div>;

  const archive = (s: Session, reset?: () => void) => {
    setError(null);
    api.archiveSession(s.id).catch((e: Error) => {
      reset?.();
      setError(e.message);
    });
  };
  return (
    <>
      <ErrorLine error={error} />
      <ul className="space-y-0.5">
        {sessions.map((s) => (
          <SessionRow
            key={s.id}
            s={s}
            scope={scopeTag(s)}
            when={s.updatedAt}
            active={isOpen(s, pathname, search)}
            onPick={onPick}
            onSwipe={{ label: t('archive.move'), icon: Archive, run: (reset) => archive(s, reset) }}
            actions={
              // touch: the row is swiped to the left instead
              <button type="button" className={`${rowAction} pointer-coarse:hidden`} title={t('archive.move')} aria-label={t('archive.move')} onClick={() => archive(s)}>
                <Archive className="size-3.5" />
              </button>
            }
          />
        ))}
      </ul>
      <p className="hidden px-3 pt-3 text-center text-xs text-muted/70 pointer-coarse:block">{t('archive.swipeHint')}</p>
    </>
  );
}

/** The archive: open a session, restore it, delete it, or empty the whole archive. */
function ArchiveList({ onPick, query = '' }: { onPick?: () => void; query?: string }) {
  const { sessions: all, loading } = useArchivedSessions();
  const scopeTag = useScopeTags(all);
  const sessions = all?.filter((s) => matchesQuery(query, s.title, scopeTag(s).name));
  const { pathname, search } = useLocation();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const act = (p: Promise<unknown>) => {
    setError(null);
    p.catch((e: Error) => setError(e.message));
  };
  const remove = async (s: Session) => {
    if (await confirmDialog({ message: t('archive.deleteOne', { title: s.title }), confirmLabel: t('common.delete'), danger: true })) act(api.deleteSession(s.id));
  };
  // "Delete all" always means the whole archive, not just what the search shows
  const emptyAll = async () => {
    if (!all?.length) return;
    if (!(await confirmDialog({ title: t('archive.emptyTitle'), message: t('archive.emptyConfirm', { n: all.length }), confirmLabel: t('archive.empty'), danger: true }))) return;
    setBusy(true);
    act(api.emptyArchive().finally(() => setBusy(false)));
  };

  if (loading && !all) return null;
  return (
    <>
      <ErrorLine error={error} />
      {all?.length ? (
        <>
          <div className="mb-2 flex items-center gap-2 px-1">
            <span className="min-w-0 flex-1 text-xs text-muted">{t('archive.count', { n: all.length })}</span>
            <Button size="sm" variant="danger" loading={busy} onClick={() => void emptyAll()}>
              <Trash2 className="size-3.5" /> {t('archive.empty')}
            </Button>
          </div>
          {!sessions?.length ? <div className="px-3 py-2 text-sm text-muted">{t('sessions.nothingFound')}</div> : null}
          <ul className="space-y-0.5">
            {sessions?.map((s) => (
              <SessionRow
                key={s.id}
                s={s}
                scope={scopeTag(s)}
                when={s.archivedAt ?? s.updatedAt}
                active={isOpen(s, pathname, search)}
                onPick={onPick}
                actions={
                  <>
                    <button type="button" className={rowAction} title={t('archive.restore')} aria-label={t('archive.restore')} onClick={() => act(api.restoreSession(s.id))}>
                      <ArchiveRestore className="size-3.5" />
                    </button>
                    <button type="button" className={`${rowAction} hover:text-danger`} title={t('common.delete')} aria-label={t('common.delete')} onClick={() => void remove(s)}>
                      <Trash2 className="size-3.5" />
                    </button>
                  </>
                }
              />
            ))}
          </ul>
        </>
      ) : (
        <div className="px-3 py-2 text-sm text-muted">{t('archive.emptyList')}</div>
      )}
    </>
  );
}

/** Right panel body: Active / Archive tabs (the chosen tab is remembered). */
function SessionsPanel({ onPick }: { onPick?: () => void }) {
  const [tab, setTab] = useLocalStorage<'active' | 'archive'>('hub.history.tab', 'active');
  const [query, setQuery] = useState('');
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 border-b border-line">
        {(['active', 'archive'] as const).map((id) => (
          <button
            key={id}
            type="button"
            onClick={() => setTab(id)}
            className={`flex flex-1 items-center justify-center gap-1.5 border-b-2 py-2 text-sm pointer-coarse:py-3 ${tab === id ? 'border-accent text-fg' : 'border-transparent text-muted hover:text-fg'}`}
          >
            {id === 'archive' ? <Archive className="size-3.5" /> : null}
            {t(id === 'active' ? 'archive.tabActive' : 'archive.tabArchive')}
          </button>
        ))}
      </div>
      <div className="shrink-0 px-2.5 pt-2.5">
        <SessionSearch value={query} onChange={setQuery} />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-2.5">{tab === 'active' ? <History onPick={onPick} query={query} /> : <ArchiveList onPick={onPick} query={query} />}</div>
    </div>
  );
}

function Wordmark({ className = 'h-5' }: { className?: string }) {
  return <img src="/brand/logo-wordmark.svg" alt="hubness" className={className} draggable={false} />;
}

export function Layout() {
  // one live connection keeps every cached session list current; the agent list feeds names and pickers
  useSessionSync();
  useAgents();
  const { pathname, search } = useLocation();
  // a screen can also ask for the whole phone screen itself (usePhoneFullscreen)
  const [fullReq, setFullscreen] = useState(false);
  const full = isFullscreen(pathname, search) || fullReq;
  const [expanded, setExpanded] = useLocalStorage('hub.sidebar', false);
  // Desktop: the right panel state is remembered; phone: it opens on top and closes on navigation
  const [panel, setPanel] = useLocalStorage('hub.history', false);
  const [drawer, setDrawer] = useState(false);
  const historyOpen = isDesktop() ? panel : drawer;
  // desktop panel width: drag its left edge (remembered)
  const [panelWidth, setPanelWidth] = useLocalStorage('hub.history.width', 340);
  const startPanelResize = (e: ReactPointerEvent) => {
    const x0 = e.clientX;
    const w0 = panelWidth;
    document.body.style.userSelect = 'none';
    document.body.style.cursor = 'col-resize';
    const move = (ev: PointerEvent) => setPanelWidth(Math.min(640, Math.max(260, w0 + x0 - ev.clientX)));
    const up = () => {
      document.body.style.userSelect = '';
      document.body.style.cursor = '';
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  const toggleHistory = useCallback(() => {
    if (isDesktop()) setPanel(!panel);
    else setDrawer((d) => !d);
  }, [panel, setPanel]);
  const openHistory = useCallback(() => {
    if (isDesktop()) setPanel(true);
    else setDrawer(true);
  }, [setPanel]);

  useEffect(() => setDrawer(false), [pathname]);
  const nav = useNavigate();
  const auth = useAuth();
  const access = useAccess();
  // System and Harness only for accounts that have them
  const navItems = NAV.filter((n) => (n.to === '/system' ? access.can('system') : n.to === '/harness' ? access.can('harness') : true));
  useKeyActions('nav', [
    ...navItems.map((n) => ({ key: n.key, label: t(n.label), section: t('keys.navigate'), run: () => nav(n.to) })),
    { key: 'H', label: t('history.title'), section: t('keys.navigate'), run: toggleHistory },
    { key: '?', label: t('keys.settings'), section: t('keys.navigate'), run: () => nav('/settings/keyboard') },
  ]);
  // phone drawer: slides out to the right on tap outside / ×, follows the finger when swiped away
  const drawerRef = useRef<HTMLElement>(null);
  const backdropRef = useRef<HTMLDivElement>(null);
  const closeDrawer = useCallback(() => {
    if (backdropRef.current) backdropRef.current.style.opacity = '0';
    slideOut(drawerRef.current, 1, () => setDrawer(false));
  }, []);
  const drawerSwipe = useSwipe({
    dir: 1,
    target: () => drawerRef.current,
    threshold: 0.3,
    onProgress: (p) => backdropRef.current && (backdropRef.current.style.opacity = String(1 - p)),
    onCommit: () => setDrawer(false),
  });
  useEffect(() => {
    if (!drawer) return;
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setDrawer(false);
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [drawer]);

  const panelHeader = (onClose: () => void, icon: 'close' | 'collapse') => (
    <div className="flex h-11 shrink-0 items-center gap-1 border-b border-line pr-1 pl-3 pointer-coarse:h-[52px]">
      <span className="font-semibold text-accent">{t('history.title')}</span>
      <Link to="/" className="tap ml-auto inline-flex size-9 items-center justify-center rounded-lg hover:bg-sunken" title={t('history.newSession')} aria-label={t('history.newSession')}>
        <SquarePen className="size-4 text-accent" />
      </Link>
      <button
        type="button"
        onClick={onClose}
        aria-label={t('history.hide')}
        title={t('history.hide')}
        className="tap inline-flex size-9 items-center justify-center rounded-lg hover:bg-sunken"
      >
        {icon === 'close' ? <X className="size-5" /> : <PanelRightClose className="size-5" />}
      </button>
    </div>
  );

  return (
    <NavContext.Provider value={{ historyOpen, toggleHistory, openHistory, setFullscreen }}>
      <LeaderOverlay />
      <div className="flex h-app">
        {/* Desktop: sidebar, either a narrow strip or expanded with history */}
        <nav
          className={`hidden shrink-0 flex-col border-r border-line bg-surface transition-[width] duration-150 lg:flex ${expanded ? 'w-60' : 'w-12'}`}
        >
          <div className={`flex h-11 shrink-0 items-center gap-2 border-b border-line ${expanded ? 'px-2.5' : 'justify-center'}`}>
            <button
              type="button"
              onClick={() => setExpanded(!expanded)}
              aria-label={t('nav.menu')}
              title={t('nav.menu')}
              className="inline-flex size-9 items-center justify-center rounded-lg text-fg/80 hover:bg-sunken hover:text-fg"
            >
              <Menu className="size-5" />
            </button>
            {expanded ? (
              <Link to="/" aria-label="hubness" className="ml-auto pr-1.5">
                <Wordmark />
              </Link>
            ) : null}
          </div>
          <div className={`space-y-px pt-2 ${expanded ? 'px-0' : 'px-0'}`}>
            {navItems.map(({ to, label, icon: Icon, key }) => (
              <NavLink
                key={to}
                to={to}
                end={to === '/'}
                title={expanded ? undefined : t(label)}
                className={({ isActive }) =>
                  `relative flex items-center gap-3 border-l-2 py-2 ${expanded ? 'px-3' : 'justify-center'} ${isActive ? 'border-accent bg-sunken text-accent' : 'border-transparent text-muted hover:bg-sunken hover:text-fg'}`
                }
              >
                <JumpLabel k={key} />
                <Icon className="size-4 shrink-0" />
                {expanded ? t(label) : null}
              </NavLink>
            ))}
          </div>
          {auth.user ? (
            <button
              type="button"
              onClick={() => void auth.logout()}
              title={expanded ? undefined : `${t('auth.logout')} (${auth.user.username})`}
              className={`mt-auto flex items-center gap-3 border-t border-line py-2.5 text-muted hover:bg-sunken hover:text-fg ${expanded ? 'px-3' : 'justify-center'}`}
            >
              <LogOut className="size-4 shrink-0" />
              {expanded ? <span className="min-w-0 truncate text-sm">{t('auth.logout')} · {auth.user.username}</span> : null}
            </button>
          ) : null}
        </nav>

        <div className="flex min-w-0 flex-1 flex-col">
          <main className="flex min-h-0 flex-1 flex-col overflow-x-hidden overflow-y-auto">
            <Suspense fallback={<PageSpinner />}>
              <Outlet />
            </Suspense>
          </main>
          {/* Phone: bottom navigation */}
          {!full ? (
            <nav className="ui-tabbar hide-on-kb pb-safe grid shrink-0 grid-cols-5 border-t border-line bg-surface lg:hidden">
              {navItems.map(({ to, label, icon: Icon }) => (
                <NavLink
                  key={to}
                  to={to}
                  end={to === '/'}
                  className={({ isActive }) =>
                    `ui-tab flex flex-col items-center gap-0.5 border-t-2 py-1.5 text-[11px] ${isActive ? 'border-accent text-accent' : 'border-transparent text-muted'}`
                  }
                >
                  <Icon className="size-[22px]" />
                  {t(label)}
                </NavLink>
              ))}
            </nav>
          ) : null}
        </div>

        {/* Desktop: right panel with all sessions */}
        {panel ? (
          <aside className="relative hidden shrink-0 flex-col border-l border-line bg-surface lg:flex" style={{ width: panelWidth }}>
            <div
              onPointerDown={startPanelResize}
              className="absolute inset-y-0 -left-1 z-10 w-2 cursor-col-resize hover:bg-accent/30"
              title={t('ws.resize')}
            />
            {panelHeader(() => setPanel(false), 'collapse')}
            <SessionsPanel />
          </aside>
        ) : null}

        {/* Phone: sessions panel slides in from the right */}
        {drawer ? (
          <div className="inset-app z-50 lg:hidden" onClick={closeDrawer}>
            <div ref={backdropRef} className="absolute inset-0 bg-black/50 transition-opacity duration-200 motion-safe:animate-fade-in" />
            {/* the whole screen on a phone (× or a swipe to the right closes it); a side panel on a tablet */}
            <aside
              ref={drawerRef}
              {...drawerSwipe}
              onClick={(e) => e.stopPropagation()}
              className="pb-safe absolute inset-y-0 right-0 flex w-full flex-col bg-surface motion-safe:animate-slide-in-right sm:w-[86vw] sm:max-w-lg sm:border-l sm:border-line"
            >
              {panelHeader(closeDrawer, 'close')}
              <SessionsPanel onPick={() => setDrawer(false)} />
            </aside>
          </div>
        ) : null}
      </div>
    </NavContext.Provider>
  );
}
