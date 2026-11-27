import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from 'react';
import { Columns2, Rows2, X } from 'lucide-react';
import * as L from '../../lib/layout';
import { t } from '../../i18n';
import { JumpLabel } from '../LeaderOverlay';
import { endDrag, getDrag, startDrag } from '../../lib/dnd';
import { type Tool } from '../RepoWorkspace';
import { ToolBadge } from './chrome';
import { Layer } from '../ui';

/** Group tab line: drag to reorder/move, middle click closes, double click keeps a preview. */
export function TabStrip({
  group,
  focused,
  canSplit,
  multiGroup,
  label,
  dirty,
  setLayout,
  onMenu,
}: {
  group: L.Group;
  focused: boolean;
  canSplit: boolean;
  multiGroup: boolean;
  label: (tab: L.Tab) => { name: string; hint: string; icon: ReactNode; badge: Tool['badge'] };
  dirty: Set<string>;
  setLayout: (fn: (l: L.Layout) => L.Layout) => void;
  onMenu: (e: ReactMouseEvent, key: string) => void;
}) {
  const [over, setOver] = useState<number | null>(null);
  const stripRef = useRef<HTMLDivElement>(null);
  // keep the active tab visible
  useEffect(() => {
    stripRef.current?.querySelector('[data-active="1"]')?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }, [group.active]);

  const drop = (index: number) => {
    const d = getDrag();
    endDrag();
    setOver(null);
    if (!d || d.kind === 'dir') return;
    if (d.kind === 'tab') setLayout((l) => L.move(l, d.from, d.key, group.id, index));
    else setLayout((l) => L.open(l, d.ref, { group: group.id, index }));
  };

  return (
    <div className="flex h-9 shrink-0 items-stretch border-b border-line bg-surface">
      <div
        ref={stripRef}
        className="flex min-w-0 flex-1 overflow-x-auto [scrollbar-width:thin]"
        onWheel={(e) => {
          // vertical wheel scrolls the tabs horizontally
          if (e.deltaY && stripRef.current) stripRef.current.scrollLeft += e.deltaY;
        }}
        onDragOver={(e) => {
          if (!getDrag()) return;
          e.preventDefault();
          if (e.target === e.currentTarget) setOver(group.tabs.length);
        }}
        onDragLeave={(e) => e.target === e.currentTarget && setOver(null)}
        onDrop={(e) => {
          e.preventDefault();
          drop(over ?? group.tabs.length);
        }}
      >
        {group.tabs.map((tab, i) => {
          const lb = label(tab);
          const active = group.active === tab.key;
          const isDirty = dirty.has(tab.key);
          return (
            <div
              key={tab.key}
              role="tab"
              aria-selected={active}
              data-active={active ? '1' : undefined}
              draggable
              title={tab.ref.kind === 'file' ? tab.ref.path : lb.name}
              onDragStart={(e) => {
                e.dataTransfer.effectAllowed = 'move';
                e.dataTransfer.setData('text/plain', lb.name);
                startDrag({ kind: 'tab', from: group.id, key: tab.key });
              }}
              onDragEnd={() => {
                endDrag();
                setOver(null);
              }}
              onDragOver={(e) => {
                if (!getDrag()) return;
                e.preventDefault();
                const r = e.currentTarget.getBoundingClientRect();
                setOver(e.clientX > r.left + r.width / 2 ? i + 1 : i);
              }}
              onClick={() => setLayout((l) => L.activate(l, group.id, tab.key))}
              onDoubleClick={() => setLayout((l) => L.pin(l, group.id, tab.key))}
              onAuxClick={(e) => e.button === 1 && setLayout((l) => L.close(l, group.id, tab.key))}
              onMouseDown={(e) => e.button === 1 && e.preventDefault()}
              onContextMenu={(e) => onMenu(e, tab.key)}
              aria-current={active ? 'page' : undefined}
              className={`ui-etab group relative flex max-w-56 shrink-0 cursor-pointer items-center gap-1.5 border-r border-line pr-1 pl-2.5 text-[13px] select-none ${
                active ? `bg-bg text-fg ${focused ? '' : 'text-fg/80'}` : 'text-muted hover:bg-sunken hover:text-fg'
              } ${over === i ? 'shadow-[inset_2px_0_0_var(--color-accent)]' : ''} ${over === i + 1 && i === group.tabs.length - 1 ? 'shadow-[inset_-2px_0_0_var(--color-accent)]' : ''}`}
            >
              {active ? <span className={`ui-etab-bar absolute inset-x-0 top-0 h-0.5 ${focused ? 'bg-accent' : 'bg-muted/50'}`} /> : null}
              <JumpLabel k={focused && i < 9 ? String(i + 1) : undefined} className="top-1 left-0.5" />
              {lb.icon}
              <span className={`truncate ${tab.preview ? 'italic' : ''}`}>{lb.name}</span>
              {lb.hint ? <span className="truncate text-[11px] text-muted">{lb.hint}</span> : null}
              <ToolBadge b={lb.badge} />
              <button
                type="button"
                aria-label={t('tabs.close')}
                title={t('tabs.close')}
                onClick={(e) => {
                  e.stopPropagation();
                  setLayout((l) => L.close(l, group.id, tab.key));
                }}
                className={`relative inline-flex size-5 shrink-0 items-center justify-center hover:bg-sunken ${active || isDirty ? '' : 'opacity-0 group-hover:opacity-100'}`}
              >
                {isDirty ? (
                  <>
                    <span className="size-2 rounded-full bg-fg group-hover:hidden" />
                    <X className="hidden size-3.5 group-hover:block" />
                  </>
                ) : (
                  <X className="size-3.5" />
                )}
              </button>
            </div>
          );
        })}
      </div>
      <div className="flex shrink-0 items-center px-0.5">
        {canSplit && group.active ? (
          <>
            <button type="button" title={`${t('tabs.split')} (Ctrl+\\)`} aria-label={t('tabs.split')} onClick={() => setLayout((l) => L.split(l, group.id, 'right'))} className="inline-flex size-7 items-center justify-center text-muted hover:bg-sunken hover:text-fg">
              <Columns2 className="size-4" />
            </button>
            <button type="button" title={`${t('tabs.splitDown')} (Ctrl+Shift+\\)`} aria-label={t('tabs.splitDown')} onClick={() => setLayout((l) => L.split(l, group.id, 'down'))} className="inline-flex size-7 items-center justify-center text-muted hover:bg-sunken hover:text-fg">
              <Rows2 className="size-4" />
            </button>
          </>
        ) : null}
        {multiGroup ? (
          <button type="button" title={t('tabs.closeGroup')} aria-label={t('tabs.closeGroup')} onClick={() => setLayout((l) => L.closeAll(l, group.id))} className="inline-flex size-7 items-center justify-center text-muted hover:bg-sunken hover:text-fg">
            <X className="size-4" />
          </button>
        ) : null}
      </div>
    </div>
  );
}

/** The main input of a tab: a chat composer, a code editor, a terminal. */
export const TAB_INPUT = '[data-autofocus], .cm-content, .xterm-helper-textarea';

/**
 * Group content: visited tabs stay mounted (hidden), so terminals/sessions/editors keep their state.
 * A tab that comes back into view gets its keyboard focus back (its own focus-on-mount ran only
 * the first time); not on a touch screen, where focus pops up the keyboard.
 */
export function GroupBody({ group, focused = true, renderTab }: { group: L.Group; focused?: boolean; renderTab: (tab: L.Tab, groupId: string) => ReactNode }) {
  const [mounted, setMounted] = useState<Set<string>>(() => new Set(group.active ? [group.active] : []));
  const bodies = useRef(new Map<string, HTMLDivElement>());
  useEffect(() => {
    if (group.active && !mounted.has(group.active)) setMounted((m) => new Set(m).add(group.active!));
  }, [group.active, mounted]);
  const shown = useRef(group.active);
  useEffect(() => {
    const prev = shown.current;
    shown.current = group.active;
    if (!group.active || prev === group.active || !focused || matchMedia('(pointer: coarse)').matches) return;
    const frame = requestAnimationFrame(() => {
      const body = bodies.current.get(group.active!);
      if (!body || body.contains(document.activeElement)) return;
      body.querySelector<HTMLElement>(TAB_INPUT)?.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, [group.active, focused]);
  return (
    <>
      {group.tabs.map((tab) =>
        mounted.has(tab.key) || tab.key === group.active ? (
          <div
            key={tab.key}
            ref={(el) => void (el ? bodies.current.set(tab.key, el) : bodies.current.delete(tab.key))}
            className={`min-h-0 flex-1 flex-col ${tab.key === group.active ? 'flex' : 'hidden'}`}
          >
            {renderTab(tab, group.id)}
          </div>
        ) : null,
      )}
    </>
  );
}

/** While a tab or a tree file is dragged: drop in the middle (into the group) or onto an edge (new split). */
export function DropZone({ group, canSplit, setLayout }: { group: L.Group; canSplit: boolean; setLayout: (fn: (l: L.Layout) => L.Layout) => void }) {
  const [zone, setZone] = useState<L.Side | 'center' | null>(null);
  const area: Record<L.Side | 'center', string> = {
    center: 'inset-0',
    left: 'inset-y-0 left-0 w-1/2',
    right: 'inset-y-0 right-0 w-1/2',
    up: 'inset-x-0 top-0 h-1/2',
    down: 'inset-x-0 bottom-0 h-1/2',
  };
  return (
    <div
      className="absolute inset-0 z-20"
      onDragOver={(e) => {
        if (!getDrag()) return;
        e.preventDefault();
        const r = e.currentTarget.getBoundingClientRect();
        const x = (e.clientX - r.left) / r.width;
        const y = (e.clientY - r.top) / r.height;
        // the nearest edge within a quarter of the size, else the middle
        const edges: [L.Side, number][] = [
          ['left', x],
          ['right', 1 - x],
          ['up', y],
          ['down', 1 - y],
        ];
        const [side, dist] = edges.sort((a, b) => a[1] - b[1])[0]!;
        setZone(canSplit && dist < 0.25 ? side : 'center');
      }}
      onDragLeave={() => setZone(null)}
      onDrop={(e) => {
        e.preventDefault();
        const d = getDrag();
        endDrag();
        if (!d || !zone || d.kind === 'dir') return;
        if (d.kind === 'tab') setLayout((l) => (zone === 'center' ? L.move(l, d.from, d.key, group.id) : L.moveToNewGroup(l, d.from, d.key, group.id, zone)));
        else setLayout((l) => (zone === 'center' ? L.open(l, d.ref, { group: group.id }) : L.openInNewGroup(l, d.ref, group.id, zone)));
      }}
    >
      {zone ? <div className={`pointer-events-none absolute border-2 border-accent/60 bg-accent/10 ${area[zone]}`} /> : null}
    </div>
  );
}

/** Right-click menu of a tab. */
export function ContextMenu({
  menu,
  layout,
  onClose,
  apply,
}: {
  menu: { x: number; y: number; group: string; key: string } | null;
  layout: L.Layout;
  onClose: () => void;
  apply: (fn: (l: L.Layout) => L.Layout) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!menu) return;
    const off = (e: Event) => {
      if (e instanceof KeyboardEvent && e.key !== 'Escape') return;
      if (e.type === 'mousedown' && ref.current?.contains(e.target as Node)) return;
      onClose();
    };
    window.addEventListener('mousedown', off);
    window.addEventListener('keydown', off);
    window.addEventListener('blur', off);
    window.addEventListener('resize', off);
    return () => {
      window.removeEventListener('mousedown', off);
      window.removeEventListener('keydown', off);
      window.removeEventListener('blur', off);
      window.removeEventListener('resize', off);
    };
  }, [menu, onClose]);
  if (!menu) return null;
  const order = L.groupOrder(layout);
  const gi = order.indexOf(menu.group);
  const g = layout.groups.find((x) => x.id === menu.group);
  const tab = g?.tabs.find((x) => x.key === menu.key);
  if (!g || !tab) return null;
  const left = order[gi - 1];
  const right = order[gi + 1];
  const items: ({ label: string; hint?: string; run: (l: L.Layout) => L.Layout; disabled?: boolean } | null)[] = [
    { label: t('tabs.close'), hint: 'Alt+W', run: (l) => L.close(l, g.id, tab.key) },
    { label: t('tabs.closeOthers'), run: (l) => L.closeOthers(l, g.id, tab.key), disabled: g.tabs.length < 2 },
    { label: t('tabs.closeRight'), run: (l) => L.closeRight(l, g.id, tab.key), disabled: g.tabs[g.tabs.length - 1]?.key === tab.key },
    { label: t('tabs.closeAll'), run: (l) => L.closeAll(l, g.id) },
    null,
    ...(tab.preview ? [{ label: t('tabs.keepOpen'), run: (l: L.Layout) => L.pin(l, g.id, tab.key) }] : []),
    { label: t('tabs.split'), hint: 'Ctrl+\\', run: (l) => L.split(l, g.id, 'right', tab.key), disabled: layout.groups.length >= L.MAX_GROUPS },
    { label: t('tabs.splitDown'), hint: 'Ctrl+Shift+\\', run: (l) => L.split(l, g.id, 'down', tab.key), disabled: layout.groups.length >= L.MAX_GROUPS },
    ...(left ? [{ label: t('tabs.moveLeft'), run: (l: L.Layout) => L.move(l, g.id, tab.key, left) }] : []),
    ...(right ? [{ label: t('tabs.moveRight'), run: (l: L.Layout) => L.move(l, g.id, tab.key, right) }] : []),
  ];
  return (
    <Layer>
      <div
        ref={ref}
        role="menu"
        className="ui-solid fixed z-50 min-w-56 border border-line bg-surface py-1 text-[13px] shadow-lg"
        style={{ left: Math.min(menu.x, window.innerWidth - 240), top: Math.min(menu.y, window.innerHeight - 280) }}
      >
        {items.map((it, i) =>
          it ? (
            <button
              key={i}
              type="button"
              role="menuitem"
              disabled={it.disabled}
              onClick={() => apply(it.run)}
              className="flex w-full items-center gap-4 px-3 py-1.5 text-left hover:bg-sunken disabled:opacity-40 disabled:hover:bg-transparent"
            >
              <span className="flex-1">{it.label}</span>
              {it.hint ? <span className="text-[11px] text-muted">{it.hint}</span> : null}
            </button>
          ) : (
            <div key={i} className="my-1 h-px bg-line" />
          ),
        )}
      </div>
    </Layer>
  );
}
