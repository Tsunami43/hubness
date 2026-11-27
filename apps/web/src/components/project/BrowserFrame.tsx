import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useNavigate } from 'react-router';
import {
  ArrowLeft,
  ArrowRight,
  Bot,
  ExternalLink,
  Fullscreen,
  Maximize2,
  Minimize2,
  Monitor,
  MousePointerClick,
  QrCode,
  RotateCcw,
  RotateCw,
  Smartphone,
  SquareTerminal,
  Tablet,
  Trash2,
  X,
} from 'lucide-react';
import type { PreviewLog, Session } from '@hubness/shared';
import { api } from '../../lib/api';
import { setAskDraft } from '../../lib/askDraft';
import { qrSvgPath } from '../../lib/qr';
import { t } from '../../i18n';
import { Button, ErrorLine, IconButton, Sheet } from '../ui';
import { Select } from '../Select';
import { useLocalStorage } from '../../lib/hooks';

type Device = 'fill' | 'phone' | 'tablet' | 'laptop';
const SIZES: Record<Exclude<Device, 'fill'>, [number, number]> = { phone: [390, 844], tablet: [820, 1180], laptop: [1366, 768] };

/** What the preview helper (packages/shared/assets/preview-inject.js) posts to us. */
type Pick = { selector: string; text: string; html: string; rect: { width: number; height: number }; viewport: { width: number; height: number }; path: string };

const LOOPBACK = ['localhost', '127.0.0.1', '[::1]', '::1'];

/**
 * The app being built, in a small browser: address bar with back / forward, device sizes,
 * console with what the page reported (errors, failed requests — also from the agent's own
 * browser), "pick an element" and "send to agent", a QR code for a real phone, full screen.
 * Talks to the page through the helper the preview proxy injects (another origin: only
 * postMessage gets through).
 */
export function BrowserFrame({ projectId, previewPort, appPort, toolbar }: { projectId: string; previewPort: number; appPort: number; toolbar?: ReactNode }) {
  const navigate = useNavigate();
  const origin = `${window.location.protocol}//${window.location.hostname}:${previewPort}`;
  // the address the agent knows the app by (its preview tools route it through the proxy themselves)
  const appOrigin = `http://localhost:${appPort}`;
  const frame = useRef<HTMLIFrameElement>(null);
  const area = useRef<HTMLDivElement>(null);
  const [src, setSrc] = useState('/');
  const [nonce, setNonce] = useState(0);
  const [path, setPath] = useState('/');
  const [draft, setDraft] = useState('/');
  const [device, setDevice] = useLocalStorage<Device>('hub.preview.device', 'fill');
  const [rotated, setRotated] = useState(false);
  const [scale, setScale] = useState(1);
  const [full, setFull] = useState(false);
  const [picking, setPicking] = useState(false);
  const [helper, setHelper] = useState(false);
  const [logs, setLogs] = useState<PreviewLog[]>([]);
  const [consoleOpen, setConsoleOpen] = useState(false);
  const [seenErrors, setSeenErrors] = useState(0);
  const [qr, setQr] = useState(false);
  const [send, setSend] = useState<string | null>(null);

  // a new target: start over
  useEffect(() => {
    setSrc('/');
    setPath('/');
    setDraft('/');
    setLogs([]);
    setSeenErrors(0);
    setHelper(false);
  }, [previewPort]);

  // the page's messages (only from our frame)
  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      const m = e.data;
      if (!m || m.hubness !== 1 || e.source !== frame.current?.contentWindow) return;
      if (m.type === 'ready') setHelper(true);
      else if (m.type === 'location') {
        setPath(m.path);
        setDraft(m.path);
      } else if (m.type === 'pick') {
        setPicking(false);
        setSend(pickDraft(m as Pick, appOrigin));
      } else if (m.type === 'pick-cancel') setPicking(false);
    };
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, [appOrigin]);

  // what the page reported, polled while the frame is shown
  useEffect(() => {
    let since = 0;
    let stop = false;
    const tick = () =>
      api.previewLogs(projectId, previewPort, since).then(
        (r) => {
          if (stop || !r.logs.length) return;
          since = r.logs[r.logs.length - 1]!.seq;
          setLogs((cur) => [...cur, ...r.logs].slice(-500));
        },
        () => {},
      );
    void tick();
    const id = setInterval(tick, 1500);
    return () => {
      stop = true;
      clearInterval(id);
    };
  }, [projectId, previewPort]);

  // fit a device frame into the area
  const size = device === 'fill' ? null : rotated ? [SIZES[device][1], SIZES[device][0]] : SIZES[device];
  useEffect(() => {
    const el = area.current;
    if (!el || !size) return setScale(1);
    const fit = () => setScale(Math.min(1, (el.clientWidth - 16) / size[0]!, (el.clientHeight - 16) / size[1]!));
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [size?.[0], size?.[1], full, consoleOpen]);

  useEffect(() => {
    if (!full) return;
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && !send && setFull(false);
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [full, send]);

  const post = (type: string) => frame.current?.contentWindow?.postMessage({ hubness: 1, type }, origin);
  // reload where the page is now (it may have navigated inside the frame)
  const reload = () => {
    setSrc(path);
    setNonce((n) => n + 1);
  };
  const go = (p: string) => {
    const next = p.startsWith('/') ? p : `/${p}`;
    setSrc(next);
    setPath(next);
    setDraft(next);
    setNonce((n) => n + 1);
  };
  const togglePick = () => {
    post(picking ? 'pick-cancel' : 'pick');
    setPicking(!picking);
  };

  const errors = logs.filter((l) => l.level === 'error' || l.level === 'network').length;
  const unseen = consoleOpen ? 0 : errors - seenErrors;
  const openConsole = () => {
    setConsoleOpen(!consoleOpen);
    setSeenErrors(errors);
  };
  const remote = !LOOPBACK.includes(window.location.hostname);

  return (
    <div className={full ? 'inset-app pt-safe fixed z-40 flex flex-col bg-bg' : 'flex min-h-0 flex-1 flex-col'}>
      <div className="flex shrink-0 flex-wrap items-center gap-1 border-b border-line px-1.5 py-1">
        {toolbar}
        <IconButton label={t('preview.back')} onClick={() => post('back')} disabled={!helper} className="size-7">
          <ArrowLeft className="size-3.5" />
        </IconButton>
        <IconButton label={t('preview.forward')} onClick={() => post('forward')} disabled={!helper} className="size-7">
          <ArrowRight className="size-3.5" />
        </IconButton>
        <IconButton label={t('preview.reload')} onClick={reload} className="size-7">
          <RotateCw className="size-3.5" />
        </IconButton>
        <form
          className="flex min-w-32 flex-1 items-center border border-line bg-bg px-2"
          onSubmit={(e) => {
            e.preventDefault();
            go(draft);
          }}
        >
          <input value={draft} onChange={(e) => setDraft(e.target.value)} className="min-w-0 flex-1 bg-transparent px-1 py-0.5 font-mono text-[12.5px] outline-none" spellCheck={false} autoCapitalize="off" aria-label={t('preview.address')} />
        </form>
        <div className="flex items-center gap-0.5">
          {(
            [
              ['fill', Maximize2, t('preview.deviceFill')],
              ['phone', Smartphone, t('preview.devicePhone')],
              ['tablet', Tablet, t('preview.deviceTablet')],
              ['laptop', Monitor, t('preview.deviceLaptop')],
            ] as const
          ).map(([id, Icon, label]) => (
            <IconButton key={id} label={label} onClick={() => setDevice(id)} className={`size-7 ${device === id ? 'bg-sunken text-accent' : ''}`}>
              <Icon className="size-3.5" />
            </IconButton>
          ))}
          {device !== 'fill' ? (
            <IconButton label={t('preview.rotate')} onClick={() => setRotated(!rotated)} className="size-7">
              <RotateCcw className="size-3.5" />
            </IconButton>
          ) : null}
        </div>
        <IconButton label={t('preview.pick')} onClick={togglePick} disabled={!helper} className={`size-7 ${picking ? 'bg-accent text-accent-fg hover:bg-accent hover:text-accent-fg' : ''}`}>
          <MousePointerClick className="size-3.5" />
        </IconButton>
        <IconButton label={t('preview.console')} onClick={openConsole} className={`relative size-7 ${consoleOpen ? 'bg-sunken text-accent' : ''}`}>
          <SquareTerminal className="size-3.5" />
          {unseen > 0 ? <span className="absolute -top-0.5 -right-0.5 min-w-4 rounded-full bg-danger px-1 text-[10px] leading-4 font-semibold text-white">{unseen > 99 ? '99+' : unseen}</span> : null}
        </IconButton>
        <IconButton label={t('preview.qr')} onClick={() => setQr(true)} className="size-7">
          <QrCode className="size-3.5" />
        </IconButton>
        <a href={origin + path} target="_blank" rel="noopener" title={t('preview.newTab')} className="ui-icon inline-flex size-7 items-center justify-center text-muted hover:bg-sunken hover:text-fg">
          <ExternalLink className="size-3.5" />
        </a>
        <IconButton label={full ? t('preview.exitFull') : t('preview.full')} onClick={() => setFull(!full)} className="size-7">
          {full ? <Minimize2 className="size-3.5" /> : <Fullscreen className="size-3.5" />}
        </IconButton>
      </div>
      {picking ? <div className="shrink-0 bg-accent/15 px-3 py-1 text-xs text-accent">{t('preview.pickHint')}</div> : null}

      <div ref={area} className={`relative min-h-0 flex-1 overflow-hidden ${size ? 'flex items-start justify-center bg-sunken p-2' : ''}`}>
        {/* one element in both modes: switching devices keeps the page where it is */}
        <div style={size ? { width: size[0]! * scale, height: size[1]! * scale } : undefined} className={size ? 'shrink-0' : 'h-full w-full'}>
          <iframe
            ref={frame}
            key={`${previewPort}#${nonce}`}
            src={origin + src}
            title={t('preview.title')}
            style={size ? { width: size[0], height: size[1], transform: `scale(${scale})`, transformOrigin: '0 0' } : undefined}
            className={size ? 'rounded-md border border-line bg-white shadow-lg' : 'h-full w-full bg-white'}
          />
        </div>
        {size ? <div className="absolute right-2 bottom-1 font-mono text-[10px] text-muted">{`${size[0]}×${size[1]}${scale < 1 ? ` · ${Math.round(scale * 100)}%` : ''}`}</div> : null}
      </div>

      {consoleOpen ? (
        <ConsoleDrawer
          logs={logs}
          onClose={() => setConsoleOpen(false)}
          onClear={() => {
            setLogs([]);
            setSeenErrors(0);
            void api.clearPreviewLogs(projectId, previewPort).catch(() => {});
          }}
          onAsk={() => setSend(logsDraft(logs, appOrigin + path))}
        />
      ) : null}

      {qr ? <QrSheet url={origin + path} remote={remote} onClose={() => setQr(false)} /> : null}
      {send !== null ? (
        <SendToAgent
          projectId={projectId}
          initial={send}
          onClose={() => setSend(null)}
          onNewChat={(text) => {
            setAskDraft(text, projectId);
            navigate('/');
          }}
        />
      ) : null}
    </div>
  );
}

const LEVEL: Record<PreviewLog['level'], string> = { error: 'text-danger', network: 'text-warn', warn: 'text-warn', log: 'text-muted' };

function ConsoleDrawer({ logs, onClose, onClear, onAsk }: { logs: PreviewLog[]; onClose: () => void; onClear: () => void; onAsk: () => void }) {
  const [onlyErrors, setOnlyErrors] = useState(false);
  const list = onlyErrors ? logs.filter((l) => l.level === 'error' || l.level === 'network') : logs;
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => end.current?.scrollIntoView({ block: 'end' }), [list.length]);
  return (
    <div className="flex h-[38%] min-h-32 shrink-0 flex-col border-t border-line bg-surface">
      <div className="flex shrink-0 items-center gap-1.5 border-b border-line/60 px-2 py-1">
        <span className="text-xs font-medium">{t('preview.console')}</span>
        <button type="button" onClick={() => setOnlyErrors(!onlyErrors)} className={`rounded-md px-1.5 py-0.5 text-[11px] ${onlyErrors ? 'bg-sunken text-fg' : 'text-muted hover:text-fg'}`}>
          {t('preview.onlyErrors')}
        </button>
        <span className="flex-1" />
        <Button size="sm" onClick={onAsk} disabled={!logs.length}>
          <Bot className="size-3.5" />
          {t('preview.askAgent')}
        </Button>
        <IconButton label={t('preview.clear')} onClick={onClear} className="size-7">
          <Trash2 className="size-3.5" />
        </IconButton>
        <IconButton label={t('common.close')} onClick={onClose} className="size-7">
          <X className="size-3.5" />
        </IconButton>
      </div>
      <div className="min-h-0 flex-1 overflow-auto font-mono text-[11.5px]">
        {list.length ? (
          list.map((l) => (
            <div key={`${l.source}${l.seq}`} className="flex gap-2 border-b border-line/40 px-2 py-0.5">
              <span className={`w-14 shrink-0 ${LEVEL[l.level]}`}>{l.level}</span>
              {l.source === 'agent' ? <span className="shrink-0 rounded bg-accent/15 px-1 text-[10px] text-accent">{t('preview.fromAgent')}</span> : null}
              <span className="min-w-0 flex-1 break-all whitespace-pre-wrap">{l.text}</span>
            </div>
          ))
        ) : (
          <div className="p-3 font-sans text-xs text-muted">{t('preview.noLogs')}</div>
        )}
        <div ref={end} />
      </div>
    </div>
  );
}

function QrSheet({ url, remote, onClose }: { url: string; remote: boolean; onClose: () => void }) {
  const q = useMemo(() => {
    try {
      return qrSvgPath(url);
    } catch {
      return null;
    }
  }, [url]);
  return (
    <Sheet open onClose={onClose} title={t('preview.qr')}>
      <div className="flex flex-col items-center gap-3 text-center">
        {q ? (
          <svg viewBox={`0 0 ${q.size} ${q.size}`} className="size-64 rounded-lg bg-white" shapeRendering="crispEdges" role="img" aria-label={url}>
            <path d={q.d} fill="#000" />
          </svg>
        ) : null}
        <code className="font-mono text-xs break-all">{url}</code>
        <p className="text-xs text-muted">{remote ? t('preview.qrHint') : t('preview.qrLocal')}</p>
      </div>
    </Sheet>
  );
}

/** A message for the agent about a picked element. */
function pickDraft(p: Pick, origin: string): string {
  const lines = [t('preview.pickIntro', { url: origin + p.path, width: p.viewport.width, height: p.viewport.height }), `- selector: \`${p.selector}\``];
  if (p.text) lines.push(`- text: “${p.text.replace(/\s+/g, ' ').slice(0, 200)}”`);
  lines.push(`- size: ${p.rect.width}×${p.rect.height}`, '```html', p.html, '```');
  return lines.join('\n') + '\n\n';
}

function logsDraft(logs: PreviewLog[], url: string): string {
  const list = logs.filter((l) => l.level !== 'log').slice(-60);
  const body = (list.length ? list : logs.slice(-60)).map((l) => `[${l.level}] ${l.text}`).join('\n').slice(-15_000);
  return `${t('preview.logsIntro', { url })}\n\`\`\`\n${body}\n\`\`\`\n\n`;
}

/** The draft, and where it goes: a session of this project, or a new chat. */
function SendToAgent({ projectId, initial, onClose, onNewChat }: { projectId: string; initial: string; onClose: () => void; onNewChat: (text: string) => void }) {
  const navigate = useNavigate();
  const [text, setText] = useState(initial);
  const [sessions, setSessions] = useState<Session[] | null>(null);
  const [target, setTarget] = useState('new');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    api.sessions({ projectId }).then((list) => {
      const recent = [...list].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)).slice(0, 15);
      setSessions(recent);
      if (recent[0]) setTarget(recent[0].id);
    }, () => setSessions([]));
    requestAnimationFrame(() => {
      const el = ref.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
      el.scrollTop = el.scrollHeight;
    });
  }, [projectId]);

  const submit = async () => {
    if (target === 'new') return onNewChat(text);
    setBusy(true);
    setError(null);
    try {
      await api.send(target, text);
      onClose();
      navigate(`/projects/${projectId}/s/${target}`);
    } catch (e) {
      setError((e as Error).message);
      setBusy(false);
    }
  };

  return (
    <Sheet
      open
      onClose={onClose}
      title={t('preview.sendTitle')}
      wide
      footer={
        <>
          <Select
            size="sm"
            value={target}
            onChange={setTarget}
            className="max-w-64 min-w-0 flex-1"
            options={[{ value: 'new', label: t('preview.newChat') }, ...(sessions ?? []).map((s) => ({ value: s.id, label: s.title || s.id.slice(0, 8) }))]}
          />
          <span className="flex-1" />
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button variant="primary" loading={busy} disabled={!text.trim()} onClick={() => void submit()}>
            {target === 'new' ? t('preview.toNewChat') : t('preview.send')}
          </Button>
        </>
      }
    >
      <textarea
        ref={ref}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => (e.metaKey || e.ctrlKey) && e.key === 'Enter' && text.trim() && void submit()}
        className="ui-input min-h-64 w-full border border-line bg-bg p-2.5 font-mono text-[12.5px] outline-none focus:border-accent"
        placeholder={t('preview.sendPlaceholder')}
      />
      <p className="mt-1 text-xs text-muted">{t('preview.sendHint')}</p>
      <ErrorLine error={error} />
    </Sheet>
  );
}
