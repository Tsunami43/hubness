import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import '@xterm/xterm/css/xterm.css';
import { Check, Copy, CornerDownLeft, ExternalLink, Square } from 'lucide-react';
import type { JobInfo, JobStreamMsg } from '@hubness/shared';
import { api } from '../lib/api';
import { t } from '../i18n';
import { Button, Sheet, Spinner } from './ui';
import { TerminalAskBar } from './SelectionAsk';

/** Terminal colors come from theme tokens (nothing hardcoded in the component). */
function themeColors() {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string) => css.getPropertyValue(`--color-${name}`).trim();
  return {
    background: v('surface'),
    foreground: v('fg'),
    cursor: v('accent'),
    selectionBackground: v('line'),
    red: v('danger'),
    green: v('ok'),
    yellow: v('warn'),
    blue: v('fg'),
    cyan: v('fg'),
    brightBlack: v('muted'),
  };
}

/** Keys for phone: arrows and Enter are needed by interactive CLI menus. */
const KEYS: [string, string][] = [
  ['↑', '\x1b[A'],
  ['↓', '\x1b[B'],
  ['Tab', '\t'],
  ['Esc', '\x1b'],
  ['⏎', '\r'],
  ['Ctrl+C', '\x03'],
];

const URL_RE = /https?:\/\/[^\s\x1b\x07"'<>]+/g;
const strip = (s: string) => s.replace(/\x1b\][^\x07]*(\x07|\x1b\\)/g, '').replace(/\x1b\[[0-9;?]*[a-zA-Z]/g, '');

type Status = { running: boolean; code: number | null };

/**
 * Task terminal (embeddable): output in xterm.js, keystrokes go back
 * to the pty. Reconnects by task id: the server sends the accumulated output.
 * On phone: an input field and a row of keys (arrows, Tab, Esc, Ctrl+C).
 */
export function JobTerminal({
  job,
  className = 'h-[46dvh] sm:h-[420px]',
  onStatus,
  onText,
  onExit,
  mobileKeys = true,
  fill = false,
}: {
  job: JobInfo;
  className?: string;
  /** Stretch to the parent's full height (tool panel). */
  fill?: boolean;
  onStatus?: (s: Status) => void;
  /** All output without ANSI (for finding links) and raw. */
  onText?: (plain: string, raw: string) => void;
  onExit?: () => void;
  mobileKeys?: boolean;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [status, setStatus] = useState<Status>({ running: true, code: null });
  const [line, setLine] = useState('');
  const [selection, setSelection] = useState('');
  // the end of the selection (the edge where the mouse let go): the ask bar shows up right there
  const wrap = useRef<HTMLDivElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  /** The selection's end cell, relative to the wrapper: its x and the top/bottom of its row. */
  const [at, setAt] = useState<{ x: number; top: number; bottom: number; above: boolean } | null>(null);
  const termRef = useRef<XTerm | null>(null);
  const cb = useRef({ onStatus, onText, onExit });
  cb.current = { onStatus, onText, onExit };

  useEffect(() => {
    if (!box.current) return;
    const set = (s: Status) => {
      setStatus(s);
      cb.current.onStatus?.(s);
    };
    set({ running: true, code: null });
    const term = new XTerm({
      convertEol: false,
      cursorBlink: true,
      fontFamily: '"Fira Code", ui-monospace, monospace',
      fontSize: 13,
      scrollback: 5000,
      theme: themeColors(),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon((_e, uri) => window.open(uri, '_blank', 'noopener')));
    term.open(box.current);
    termRef.current = term;
    fit.fit();
    // terminal selection → "ask the agent" (panel above the terminal)
    const selSub = term.onSelectionChange(() => setSelection(term.getSelection()));

    let raw = '';
    let plain = '';
    const write = (data: string) => {
      term.write(data);
      raw = (raw + data).slice(-40000);
      plain = (plain + strip(data)).slice(-20000);
      cb.current.onText?.(plain, raw);
    };

    // keystrokes are buffered and sent in batches
    let pending = '';
    let timer: ReturnType<typeof setTimeout> | null = null;
    const sub = term.onData((raw) => {
      // the server answers Primary Device Attributes itself (also for replayed output)
      const d = raw.replace(/\x1b\[\?[\d;]*c/g, '');
      if (!d) return;
      pending += d;
      if (!timer)
        timer = setTimeout(() => {
          const data = pending;
          pending = '';
          timer = null;
          api.jobInput(job.id, data).catch(() => {});
        }, 15);
    });

    const es = new EventSource(`/api/jobs/${job.id}/stream`);
    const finish = (code: number | null, note: boolean) => {
      set({ running: false, code });
      if (note) term.write(`\r\n\x1b[2m[${code === null ? t('term.stopped') : t('term.exited', { code })}]\x1b[0m\r\n`);
      es.close();
      cb.current.onExit?.();
    };
    es.onmessage = (ev) => {
      const m = JSON.parse(ev.data) as JobStreamMsg;
      if (m.type === 'snapshot') {
        write(m.output);
        if (m.job.status === 'exited') finish(m.job.exitCode, false);
      } else if (m.type === 'output') write(m.data);
      else if (m.type === 'exit') finish(m.code, true);
    };
    es.onerror = () => {
      // the task was deleted (server restarted), stop reconnecting
      if (es.readyState === EventSource.CLOSED) set({ running: false, code: null });
    };
    const ro = new ResizeObserver(() => fit.fit());
    ro.observe(box.current);
    term.focus();
    return () => {
      ro.disconnect();
      es.close();
      sub.dispose();
      selSub.dispose();
      term.dispose();
    };
  }, [job.id]);

  // the button may be let go outside the terminal: listen on the window
  useEffect(() => {
    if (!dragging) return;
    const up = (e: PointerEvent) => {
      setDragging(false);
      setAt(selectionEdge(e.clientX, e.clientY));
    };
    window.addEventListener('pointerup', up);
    return () => window.removeEventListener('pointerup', up);
  }, [dragging]);

  /**
   * Pixel box of the selection edge nearest to the pointer (a drag can go either way):
   * cells → pixels by the screen size over cols/rows, rows relative to the scrolled viewport.
   */
  const selectionEdge = (px: number, py: number) => {
    const term = termRef.current;
    const pos = term?.getSelectionPosition();
    const screen = box.current?.querySelector('.xterm-screen')?.getBoundingClientRect();
    const r = wrap.current?.getBoundingClientRect();
    if (!term || !pos || !screen || !r) return null;
    const cw = screen.width / term.cols;
    const ch = screen.height / term.rows;
    const box_ = (c: { x: number; y: number }) => {
      const row = c.y - term.buffer.active.viewportY;
      return { x: screen.left + c.x * cw, top: screen.top + row * ch, bottom: screen.top + (row + 1) * ch };
    };
    const a = box_(pos.start);
    const b = box_(pos.end);
    const d = (p: typeof a) => Math.hypot(p.x - px, (p.top + p.bottom) / 2 - py);
    // ended at the top (dragged upwards): the bar goes above, not over the selected text
    const atStart = d(a) < d(b);
    const e = atStart ? a : b;
    // the edge may be scrolled out of view: keep it inside the screen
    const clampY = (y: number) => Math.max(screen.top, Math.min(screen.bottom, y));
    return { x: e.x - r.left, top: clampY(e.top) - r.top, bottom: clampY(e.bottom) - r.top, above: atStart };
  };

  // keep the bar inside the terminal: below the pointer, or above it near the bottom edge
  useLayoutEffect(() => {
    const el = bar.current;
    const w = wrap.current;
    if (!el || !w || !at) return;
    const W = w.clientWidth;
    const H = w.clientHeight;
    const { offsetWidth: bw, offsetHeight: bh } = el;
    // starts at the edge: under its row (over it for an upward selection or near the bottom)
    el.style.left = `${Math.max(4, Math.min(at.x - 12, W - bw - 4))}px`;
    const below = at.bottom + 4;
    const over = at.top - bh - 4;
    el.style.top = `${(at.above && over >= 4) || below + bh > H - 4 ? Math.max(4, over) : below}px`;
  });

  const send = () => {
    void api.jobInput(job.id, line + '\r').catch(() => {});
    setLine('');
  };

  return (
    <div className={`flex min-h-0 flex-col gap-2 ${fill ? 'flex-1' : ''}`}>
      {/* the ask bar floats over the terminal: taking room would resize it (refit) mid-selection */}
      <div
        ref={wrap}
        className={`relative ${fill ? 'flex min-h-0 flex-1 flex-col' : ''}`}
        onPointerDown={(e) => {
          if (e.button !== 0 || bar.current?.contains(e.target as Node)) return;
          setDragging(true);
          setAt(null);
        }}
      >
        <div ref={box} className={`overflow-hidden border border-line bg-surface p-1.5 ${fill ? 'min-h-0 flex-1' : className}`} />
        {selection.trim() && !dragging ? (
          <div ref={bar} className="absolute z-10" style={at ? { left: at.x, top: at.bottom } : { top: 6, right: 16 }}>
            <TerminalAskBar text={selection} title={job.title} className="border border-line bg-surface px-2 py-1 shadow-xl shadow-black/40" />
          </div>
        ) : null}
      </div>
      {status.running && mobileKeys ? (
        <>
          <form
            className="flex gap-2 lg:hidden"
            onSubmit={(e) => {
              e.preventDefault();
              send();
            }}
          >
            <input
              value={line}
              onChange={(e) => setLine(e.target.value)}
              placeholder={t('term.inputPlaceholder')}
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              className="min-w-0 flex-1 rounded-lg border border-line bg-bg px-3 py-2 font-mono text-sm outline-none focus:border-accent"
            />
            <Button type="submit" size="sm" title={t('term.send')}>
              <CornerDownLeft className="size-4" />
            </Button>
          </form>
          <div className="flex flex-wrap gap-1.5 lg:hidden">
            {KEYS.map(([label, seq]) => (
              <button
                key={label}
                type="button"
                onClick={() => void api.jobInput(job.id, seq).catch(() => {})}
                className="min-w-10 rounded-lg border border-line bg-bg px-2.5 py-1.5 font-mono text-sm text-muted hover:text-fg"
              >
                {label}
              </button>
            ))}
          </div>
        </>
      ) : null}
    </div>
  );
}

/** Task status for the footer: running / done / stopped / error. */
export function JobStatusLine({ status }: { status: Status }) {
  return (
    <div className="flex min-w-0 flex-1 items-center gap-2 text-sm">
      {status.running ? (
        <>
          <Spinner /> <span className="text-muted">{t('term.running')}</span>
        </>
      ) : status.code === 0 ? (
        <span className="text-ok">{t('term.done')}</span>
      ) : status.code === null ? (
        <span className="text-muted">{t('term.stopped')}</span>
      ) : (
        <span className="text-danger">{t('term.failed', { code: status.code ?? '—' })}</span>
      )}
    </div>
  );
}

/**
 * Sheet for a terminal task (install, login, clone): terminal + an
 * "open link" button for URL-based login.
 */
export function TerminalSheet({ job, onClose, onDone }: { job: JobInfo | null; onClose: () => void; onDone?: () => void }) {
  const [status, setStatus] = useState<Status>({ running: true, code: null });
  const [link, setLink] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  useEffect(() => setLink(null), [job?.id]);

  return (
    <Sheet
      open={Boolean(job)}
      onClose={onClose}
      title={job?.title ?? ''}
      wide
      footer={
        <>
          <JobStatusLine status={status} />
          {status.running ? (
            <Button size="sm" onClick={() => job && api.jobKill(job.id)}>
              <Square className="size-3.5" /> {t('term.stop')}
            </Button>
          ) : (
            <Button size="sm" variant="primary" onClick={onClose}>
              {t('common.close')}
            </Button>
          )}
        </>
      }
    >
      <div className="space-y-3">
        <div className="font-mono text-xs break-all text-muted">$ {job?.command}</div>
        {link && status.running ? (
          <div className="flex flex-wrap items-center gap-2 rounded-xl border border-accent/40 bg-accent/8 px-3 py-2.5">
            <span className="min-w-0 basis-full text-sm sm:flex-1 sm:basis-auto">{t('term.linkHint')}</span>
            <a href={link} target="_blank" rel="noopener" className="btn-accent inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-sm">
              <ExternalLink className="size-3.5" /> {t('term.openLink')}
            </a>
            <button
              type="button"
              onClick={() =>
                void navigator.clipboard?.writeText(link).then(() => {
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1500);
                })
              }
              className="inline-flex items-center gap-1 rounded-lg border border-line px-2.5 py-1.5 text-sm text-muted hover:text-fg"
            >
              {copied ? <Check className="size-3.5 text-ok" /> : <Copy className="size-3.5" />}
              {copied ? t('harness.copied') : t('harness.copy')}
            </button>
          </div>
        ) : null}
        {job ? (
          <JobTerminal
            job={job}
            className="h-[44dvh] sm:h-[420px]"
            onStatus={setStatus}
            onExit={onDone}
            onText={(plain, raw) => {
              // login link: an OSC 8 hyperlink (that's how CLIs print it), otherwise the longest URL in the output
              const osc = [...raw.matchAll(/\x1b\]8;[^;]*;(https?:\/\/[^\x07\x1b]+)(?:\x07|\x1b\\)/g)].map((m) => m[1]!);
              const urls = osc.length ? osc : plain.match(URL_RE);
              if (urls?.length) setLink([...urls].sort((a, b) => b.length - a.length)[0]!);
            }}
          />
        ) : null}
      </div>
    </Sheet>
  );
}
