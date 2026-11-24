import { useEffect, useRef, useState, type RefObject } from 'react';
import { ChevronDown, MessageSquarePlus, MessagesSquare } from 'lucide-react';
import { useAsk } from '../lib/ask';
import { t } from '../i18n';

/**
 * "Ask the agent" buttons for a selection: into an open session or a new one. The target is
 * always named: one candidate — its title on the button; several open sessions — a menu to
 * pick from (the one in view first).
 */
export function AskActions({
  text,
  onDone,
  up = false,
  className = '',
}: {
  text: string;
  onDone?: () => void;
  /** Open the menu upwards (a bar at the bottom). */
  up?: boolean;
  className?: string;
}) {
  const api = useAsk();
  const [menu, setMenu] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!menu) return;
    const close = (e: MouseEvent) => !root.current?.contains(e.target as Node) && setMenu(false);
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && setMenu(false);
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', esc);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', esc);
    };
  }, [menu]);
  if (!api || !text.trim()) return null;
  const go = (target: string) => {
    setMenu(false);
    api.ask(text, target);
    onDone?.();
  };
  const btn = 'inline-flex min-w-0 items-center gap-1.5 px-2.5 py-1 hover:bg-sunken';
  const [one] = api.sessions;
  return (
    // mousedown must not take the focus away: that would drop the selection
    <div ref={root} className={`relative flex items-stretch ${className}`} onMouseDown={(e) => e.preventDefault()}>
      {api.sessions.length === 1 && one ? (
        <button type="button" onClick={() => go(one.id)} className={btn} title={t('ask.inSession', { title: one.title })}>
          <MessagesSquare className="size-3.5 shrink-0 text-accent" />
          <span className="truncate">{t('ask.inSession', { title: one.title })}</span>
        </button>
      ) : api.sessions.length > 1 ? (
        <button type="button" onClick={() => setMenu((m) => !m)} className={btn} aria-expanded={menu} aria-haspopup="menu">
          <MessagesSquare className="size-3.5 shrink-0 text-accent" />
          <span className="truncate">{t('ask.current')}</span>
          <ChevronDown className="size-3 shrink-0 text-muted" />
        </button>
      ) : null}
      <button
        type="button"
        onClick={() => go('new')}
        className={`${btn} shrink-0 ${api.sessions.length ? 'border-l border-line' : ''}`}
      >
        <MessageSquarePlus className="size-3.5 shrink-0 text-accent" />
        {t('ask.new')}
      </button>
      {menu ? (
        <div role="menu" className={`absolute left-0 z-50 max-h-72 w-72 max-w-[80vw] overflow-y-auto border border-line bg-surface py-1 shadow-xl shadow-black/40 ${up ? 'bottom-full mb-1' : 'top-full mt-1'}`}>
          <div className="px-3 py-1 text-[11px] text-muted">{t('ask.pick')}</div>
          {api.sessions.map((s) => (
            <button key={s.id} type="button" role="menuitem" onClick={() => go(s.id)} className="flex w-full items-center gap-2 px-3 py-1.5 text-left hover:bg-sunken">
              <MessagesSquare className="size-3.5 shrink-0 text-muted" />
              <span className="truncate">{s.title}</span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/**
 * Floating panel above selected text inside a container (read-only file view).
 * format defines how to present the selection to the agent (path, lines…).
 */
export function SelectionAsk({ container, format }: { container: RefObject<HTMLElement | null>; format: (text: string, sel: Selection) => string }) {
  const ask = useAsk();
  const [box, setBox] = useState<{ x: number; y: number; text: string } | null>(null);

  useEffect(() => {
    if (!ask) return;
    const onChange = () => {
      const sel = window.getSelection();
      const el = container.current;
      const text = sel?.toString() ?? '';
      if (!sel || !el || sel.isCollapsed || !text.trim() || !sel.anchorNode || !el.contains(sel.anchorNode)) {
        setBox(null);
        return;
      }
      const r = sel.getRangeAt(0).getBoundingClientRect();
      setBox({ x: Math.min(Math.max(8, r.left + r.width / 2), window.innerWidth - 8), y: Math.max(8, r.top - 8), text: format(text, sel) });
    };
    document.addEventListener('selectionchange', onChange);
    return () => document.removeEventListener('selectionchange', onChange);
  }, [ask, container, format]);

  if (!ask || !box) return null;
  return (
    <div className="fixed z-40 -translate-x-1/2 -translate-y-full border border-line bg-surface text-[13px] shadow-xl shadow-black/40" style={{ left: box.x, top: box.y }}>
      <AskActions
        text={box.text}
        onDone={() => {
          window.getSelection()?.removeAllRanges();
          setBox(null);
        }}
      />
    </div>
  );
}

/** Panel for the terminal (xterm has its own selection, not the DOM). */
export function TerminalAskBar({ text, title, className = '' }: { text: string; title: string; className?: string }) {
  const ask = useAsk();
  if (!ask || !text.trim()) return null;
  const payload = `${t('ask.outputFrom', { title })}\n\`\`\`\n${text.trimEnd()}\n\`\`\`\n`;
  return (
    <div className={`flex items-center gap-1 text-xs ${className}`}>
      <span className="text-muted">{t('ask.selected')}</span>
      <AskActions text={payload} />
    </div>
  );
}

/** How a code selection is put to the agent: where it is and the code itself. */
export const codeSnippet = (path: string, from: number, to: number, text: string) =>
  `${t('ask.inFile', { path, from, to })}\n\`\`\`${path.split('.').pop() ?? ''}\n${text.replace(/\n$/, '')}\n\`\`\`\n`;
