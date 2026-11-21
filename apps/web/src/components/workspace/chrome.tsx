import { type ReactNode } from 'react';
import * as L from '../../lib/layout';
import { Button } from '../ui';
import { JumpLabel } from '../LeaderOverlay';
import { ICON_COLOR, TOOL_KEYS, type Tool } from '../RepoWorkspace';

export function Title({ title, badge }: { title: string; badge?: string }) {
  return (
    <span className="inline-flex items-center gap-2">
      {title}
      {badge ? <span className="border border-line px-1 text-[11px] font-normal text-muted">{badge}</span> : null}
    </span>
  );
}

/** Icon tab; with `text` (a wide side bar) the name is written next to the icon. */
export function ActivityButton({ label, text, active, dim, badge, jump, onClick, children }: { label: string; text?: string; active: boolean; dim?: boolean; badge?: Tool['badge']; jump?: string; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      aria-pressed={active}
      onClick={onClick}
      className={`relative inline-flex min-w-0 flex-1 items-center justify-center border-b-2 ${text ? 'gap-1.5 px-2 text-[13px]' : ''} ${active ? 'border-accent text-fg' : `border-transparent ${dim ? 'text-muted/70' : 'text-muted'} hover:text-fg`}`}
    >
      {children}
      {text ? <span className="truncate">{text}</span> : null}
      <span className={text ? '' : 'absolute top-1 right-1'}>
        <ToolBadge b={badge} />
      </span>
      <JumpLabel k={jump} />
    </button>
  );
}

export function ToolBadge({ b }: { b: Tool['badge'] }) {
  if (b === 'dot') return <span className="block size-1.5 rounded-full bg-ok" />;
  if (typeof b === 'number' && b > 0) return <span className="text-[10px] leading-none text-warn">{b}</span>;
  return null;
}

/** Desktop tool row above the editor groups: always visible, highlights open tools. */
export function ToolBar({ tools, layout, onOpen }: { tools: Tool[]; layout: L.Layout; onOpen: (id: string) => void }) {
  return (
    <div className="flex h-8 shrink-0 items-center gap-0.5 overflow-x-auto border-b border-line px-1.5 [scrollbar-width:none]">
      {tools.map((x) => {
        // Shell lights up for any shell tab (each shell is its own tab)
        const match = (k: string | null) => (x.id === 'shell' ? Boolean(k?.startsWith('sh:')) : k === `t:${x.id}`);
        const at = layout.groups.some((g) => g.tabs.some((tab) => match(tab.key)));
        const shown = layout.groups.some((g) => match(g.active));
        return (
          <button
            key={x.id}
            type="button"
            onClick={() => onOpen(x.id)}
            title={x.label}
            className={`relative inline-flex h-7 shrink-0 items-center gap-1.5 px-2 text-[13px] ${shown ? 'bg-sunken text-fg' : at ? 'text-fg/80 hover:bg-sunken' : 'text-muted hover:bg-sunken hover:text-fg'}`}
          >
            <x.icon className="size-3.5" style={{ color: ICON_COLOR[x.id] }} /> {x.label}
            <ToolBadge b={x.badge} />
            <JumpLabel k={TOOL_KEYS[x.id]} />
          </button>
        );
      })}
    </div>
  );
}

export function ToolLauncher({ tools, onOpen, children }: { tools: Tool[]; onOpen: (id: string) => void; children: ReactNode }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-auto">
      <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-line px-3 py-2">
        {tools.map((x) => (
          <Button key={x.id} size="sm" variant="ghost" onClick={() => onOpen(x.id)}>
            <x.icon className="size-3.5" /> {x.label}
          </Button>
        ))}
      </div>
      {children}
    </div>
  );
}
