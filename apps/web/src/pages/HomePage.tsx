import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router';
import { ArrowRight, ArrowUp, Boxes, Check, ChevronDown, ChevronUp, FolderGit2, Loader2, MessagesSquare, Plus, Search, SlidersHorizontal } from 'lucide-react';
import type { AgentConfig, AgentKind } from '@hubness/shared';

interface Target {
  id: string;
  kind: 'project' | 'group';
  /** The system project `home` (server questions). */
  system?: boolean;
  name: string;
  /** project path or the list of the group's repositories */
  sub: string;
}
import { api } from '../lib/api';
import { permissionLabel, shortModel } from '../lib/format';
import { agentName, useAgents } from '../lib/agents';
import { useLocalStorage } from '../lib/hooks';
import { AgentPicker } from '../components/AgentPicker';
import { AgentIcon } from '../components/AgentIcon';
import { ErrorLine, HistoryButton, Sheet, StatusDot } from '../components/ui';
import { PlacesStrip } from '../components/PlacesStrip';
import { sessionPath, useNav } from '../components/Layout';
import { Link } from 'react-router';
import { agentLabel, timeAgo } from '../lib/format';
import { useRecentSessions } from '../lib/hooks';
import { useHubSettings } from '../lib/hubSettings';
import { t } from '../i18n';
import { AttachButton, createWithFiles, PendingFiles, usePendingFiles } from '../lib/attachments';
import { takeAskDraft } from '../lib/askDraft';
import { MicButton } from '../lib/voice';

interface HomeState {
  kind: AgentKind;
  projectId: string | null;
  /** Last-used settings for each agent. */
  configs: Record<AgentKind, AgentConfig>;
}

const INITIAL: HomeState = { kind: '', projectId: null, configs: {} };

const isTouch = () => window.matchMedia('(pointer: coarse)').matches;

/** Start page: logo and the "give a task" field with project, agent and model pickers. */
export function HomePage() {
  const nav = useNavigate();
  const [stored, setState] = useLocalStorage<HomeState>('hub.home', INITIAL);
  const agents = useAgents();
  const kinds = agents?.agents.map((a) => a.kind) ?? [];
  // old format, or the remembered agent is no longer registered — the hub default agent
  const base: HomeState = stored && 'configs' in stored ? stored : INITIAL;
  const state: HomeState = agents && !kinds.includes(base.kind) ? { ...base, kind: agents.defaultKind } : base;
  // a new task starts with the default permissions (Settings → Agent); harness and models are remembered
  const defaultPermission = useHubSettings()?.defaultPermission;
  useEffect(() => {
    if (!defaultPermission || !kinds.length || kinds.every((k) => state.configs[k]?.permissionMode === defaultPermission)) return;
    setState({ ...state, configs: Object.fromEntries(kinds.map((k) => [k, { ...(state.configs[k] ?? { kind: k }), permissionMode: defaultPermission }])) });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [defaultPermission, kinds.length]);
  // where to send the task: a project (one repository) or a project group
  const [projects, setProjects] = useState<Target[]>([]);
  const { openHistory, toggleHistory, historyOpen } = useNav();
  const { sessions: recent } = useRecentSessions();
  const [text, setText] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sheet, setSheet] = useState<'project' | 'model' | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);
  const [projectQuery, setProjectQuery] = useState('');
  const pickable = projects
    .filter((p) => !projectQuery.trim() || `${p.name} ${p.sub}`.toLowerCase().includes(projectQuery.trim().toLowerCase()))
    .slice(0, 100);

  useEffect(() => {
    Promise.all([api.projects(), api.groups()]).then(([ps, gs]) => {
      const names = new Map(ps.map((p) => [p.id, p.name]));
      setProjects([
        ...gs.map((g): Target => ({ id: g.id, kind: 'group', name: g.name, sub: g.projectIds.map((id) => names.get(id) ?? '?').join(', ') })),
        ...ps.map((p): Target => ({ id: p.id, kind: 'project', name: p.name, sub: p.path, system: p.kind === 'system' })),
      ]);
    }, () => {});
  }, []);

  // "Ask" from the System page: the input arrives filled in, aimed at the system project
  const [askDraft] = useState(takeAskDraft);
  useEffect(() => {
    if (!askDraft) return;
    setText(askDraft);
    requestAnimationFrame(() => {
      const el = ref.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
      el.scrollTop = el.scrollHeight;
    });
  }, [askDraft]);
  useEffect(() => {
    const home = askDraft ? projects.find((p) => p.system) : null;
    if (home && state.projectId !== home.id) setState({ ...state, projectId: home.id });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [askDraft, projects]);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(Math.max(el.scrollHeight, 52), 240)}px`;
  }, [text]);

  const available = (k: AgentKind) => agents?.agents.find((a) => a.kind === k)?.available ?? true;
  const agent: AgentConfig = state.configs[state.kind] ?? { kind: state.kind, permissionMode: 'ask' };
  const project = projects.find((p) => p.id === state.projectId) ?? null;
  const update = (patch: Partial<HomeState>) => setState({ ...state, ...patch });
  const modelLabel = shortModel(agent.model);

  const att = usePendingFiles();
  const ready = Boolean(text.trim() || att.files.length);
  const send = async () => {
    const prompt = text.trim();
    if (!ready || sending || !state.kind) return;
    setSending(true);
    setError(null);
    try {
      const s = await createWithFiles(
        {
          projectId: project?.kind === 'project' ? project.id : null,
          groupId: project?.kind === 'group' ? project.id : null,
          agent,
        },
        prompt,
        att.files,
      );
      att.clear();
      nav(sessionPath(s));
    } catch (e) {
      setError((e as Error).message);
      setSending(false);
    }
  };

  return (
    <div className="relative flex min-h-full flex-col">
      {/* the start page has no header; the sessions panel button sits where it is in other tabs' headers */}
      <HistoryButton open={historyOpen} onClick={toggleHistory} className="absolute top-2 right-2 z-10" />
      {/* open places stay reachable from the start page too (desktop) */}
      <div className="absolute top-0 right-12 left-0 z-10 hidden h-11 items-stretch px-2 lg:flex">
        <PlacesStrip />
      </div>
      <div className="flex flex-1 flex-col lg:justify-center">
      {/* Logo */}
      <div className="flex flex-1 flex-col items-center justify-center px-6 pt-10 pb-6 lg:flex-none">
        <img src="/brand/logo-hero.svg" alt="hubness" className="w-[min(460px,92%)] select-none" draggable={false} />
        <p className="mt-2 text-center text-muted">{t('home.tagline')}</p>
      </div>

      {/* Task field */}
      <div className="px-3 pb-4 lg:px-6 lg:pt-6 lg:pb-16">
        <div className="mx-auto max-w-3xl">
          <div
            {...att.dropProps}
            className={`rounded-2xl border bg-surface px-3.5 pt-3 pb-2.5 focus-within:border-accent/70 ${att.dragging ? 'border-accent bg-accent/5' : 'border-line'}`}
          >
            <PendingFiles files={att.files} onRemove={att.remove} />
            <div className="flex items-start gap-2">
              <textarea
                ref={ref}
                rows={2}
                value={text}
                autoFocus={!isTouch()}
                onPaste={att.onPaste}
                onChange={(e) => setText(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && (e.metaKey || e.ctrlKey || (!e.shiftKey && !isTouch()))) {
                    e.preventDefault();
                    void send();
                  }
                }}
                placeholder={att.dragging ? t('attach.drop') : t('home.placeholder')}
                className="min-w-0 flex-1 resize-none bg-transparent text-[16px] leading-6 outline-none [scrollbar-width:none] placeholder:text-muted/70"
              />
              <button
                type="button"
                onClick={() => setSheet('model')}
                className="flex max-w-[45%] shrink-0 items-center gap-1.5 rounded-lg px-2 py-1 text-sm text-fg/90 hover:bg-sunken"
                title={t('home.model')}
              >
                <SlidersHorizontal className="size-3.5 shrink-0" />
                <span className="truncate">{modelLabel}</span>
                <ChevronUp className="size-3.5 shrink-0 text-muted" />
              </button>
            </div>

            <div className="mt-2 flex items-center gap-2">
              <button
                type="button"
                onClick={() => setSheet('project')}
                className="flex min-w-0 items-center gap-2 rounded-lg border border-line px-2.5 py-1.5 text-sm hover:bg-sunken"
                title={t('home.project')}
              >
                {project ? project.kind === 'group' ? <Boxes className="size-4 shrink-0 text-accent" /> : <FolderGit2 className="size-4 shrink-0 text-accent" /> : <MessagesSquare className="size-4 shrink-0 text-muted" />}
                <span className="truncate">{project ? project.name : t('home.noProject')}</span>
              </button>

              <span className="ml-auto flex items-center">
                <MicButton
                  onText={(spoken) => {
                    setText((prev) => (prev.trim() ? `${prev.trimEnd()} ${spoken}` : spoken));
                    if (!isTouch()) setTimeout(() => ref.current?.focus(), 0);
                  }}
                  onError={att.setError}
                />
                <AttachButton onPick={att.add} />
              </span>
              <div>
                <HarnessSelect
                  value={state.kind}
                  kinds={kinds.filter((k) => available(k))}
                  onChange={(k) => update({ kind: k })}
                  onConnect={() => nav('/harness')}
                />
              </div>

              <button
                type="button"
                aria-label={t('session.send')}
                onClick={() => void send()}
                disabled={!ready || sending}
                className={`inline-flex size-9 shrink-0 items-center justify-center ${ready || sending ? 'btn-accent' : 'border border-line text-muted'}`}
              >
                {sending ? <Loader2 className="size-4 animate-spin" /> : <ArrowUp className="size-4" />}
              </button>
            </div>
          </div>

          <div className="mt-2 px-1 text-center text-xs text-muted">
            {project
              ? t('home.hintProject', { path: project.sub, mode: permissionLabel(agent.permissionMode ?? 'ask') })
              : t('home.hintNoProject')}
          </div>
          {att.error ? <div className="mt-2 text-center text-sm text-danger">{att.error}</div> : null}
          {error ? (
            <div className="mt-2">
              <ErrorLine error={error} />
            </div>
          ) : null}

          {/* Recent sessions: stacked cards, each slightly narrower than the previous one */}
          {recent?.length ? (
            <div className="mt-6 flex flex-col items-center gap-2">
              {recent.slice(0, 3).map((s, i) => (
                <Link
                  data-jump
                  key={s.id}
                  to={sessionPath(s)}
                  style={{ width: `${[94, 88, 82][i]}%` }}
                  className="flex items-center gap-3 rounded-xl border border-line/70 bg-surface/70 px-3.5 py-2.5 hover:border-line hover:bg-surface"
                >
                  <AgentIcon kind={s.agent.kind} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm text-fg">{s.title}</span>
                    <span className="block truncate text-xs text-muted">
                      {s.projectId || s.groupId ? (projects.find((p) => p.id === (s.projectId ?? s.groupId))?.name ?? t('history.project')) : t('history.chat')} ·{' '}
                      {agentLabel(s.agent)}
                    </span>
                  </span>
                  <span className="flex shrink-0 items-center gap-2 text-xs text-muted">
                    {timeAgo(s.updatedAt)}
                    <StatusDot status={s.status} />
                  </span>
                </Link>
              ))}
              <button
                type="button"
                onClick={openHistory}
                className="mt-1 inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-sm text-muted hover:bg-sunken hover:text-fg"
              >
                {t('history.openAll')}
                <ArrowRight className="size-3.5" />
              </button>
            </div>
          ) : null}
        </div>
      </div>

      </div>

      <Sheet open={sheet === 'project'} onClose={() => setSheet(null)} title={t('home.project')}>
        <label className="mb-2 flex items-center gap-2 rounded-lg border border-line bg-bg px-3">
          <Search className="size-4 shrink-0 text-muted" />
          <input
            value={projectQuery}
            onChange={(e) => setProjectQuery(e.target.value)}
            placeholder={t('projects.search')}
            className="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none"
            autoCapitalize="off"
            autoCorrect="off"
          />
        </label>
        <ul className="divide-y divide-line overflow-hidden rounded-xl border border-line">
          {[null, ...pickable].map((p) => (
            <li key={p?.id ?? 'none'}>
              <button
                type="button"
                onClick={() => {
                  update({ projectId: p?.id ?? null });
                  setSheet(null);
                }}
                className="flex w-full items-center gap-3 px-3.5 py-3 text-left hover:bg-sunken"
              >
                {p ? p.kind === 'group' ? <Boxes className="size-4 text-accent" /> : <FolderGit2 className="size-4 text-accent" /> : <MessagesSquare className="size-4 text-muted" />}
                <span className="min-w-0 flex-1">
                  <span className="block truncate font-medium">{p ? p.name : t('home.noProject')}</span>
                  <span className="block truncate font-mono text-xs text-muted">{p ? p.sub : t('home.noProjectHint')}</span>
                </span>
                {(p?.id ?? null) === (project?.id ?? null) ? <Check className="size-4 text-accent" /> : null}
              </button>
            </li>
          ))}
        </ul>
      </Sheet>

      <Sheet open={sheet === 'model'} onClose={() => setSheet(null)} title={`${agentName(state.kind)} · ${t('home.agentSettings')}`}>
        <AgentPicker
          value={agent}
          lockKind
          onChange={(a) => update({ configs: { ...state.configs, [state.kind]: { ...a, kind: state.kind } } })}
        />
      </Sheet>
    </div>
  );
}

/**
 * Harness picker in the task field: a button showing the selected one; on click — a list of connected
 * harnesses (installed and logged in), the last item being "connect more" (the Harness tab).
 */
function HarnessSelect({ value, kinds, onChange, onConnect }: { value: AgentKind; kinds: AgentKind[]; onChange: (k: AgentKind) => void; onConnect: () => void }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false);
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', esc);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', esc);
    };
  }, [open]);
  const item = 'flex w-full items-center gap-2 px-3 py-2 text-left text-sm hover:bg-sunken';
  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        aria-haspopup="listbox"
        aria-expanded={open}
        className="inline-flex h-9 items-center gap-2 border border-line px-2.5 text-sm hover:bg-sunken"
      >
        <AgentIcon kind={value} className="size-4" />
        <span className="hidden sm:inline">{agentName(value)}</span>
        <ChevronDown className={`size-3.5 text-muted transition ${open ? 'rotate-180' : ''}`} />
      </button>
      {open ? (
        <div role="listbox" className="absolute right-0 bottom-full z-30 mb-1 w-56 border border-line bg-surface py-1 shadow-xl shadow-black/40">
          {kinds.map((k) => (
            <button
              key={k}
              type="button"
              role="option"
              aria-selected={k === value}
              onClick={() => {
                onChange(k);
                setOpen(false);
              }}
              className={`${item} ${k === value ? 'text-accent' : ''}`}
            >
              <AgentIcon kind={k} className="size-4" />
              <span className="flex-1">{agentName(k)}</span>
              {k === value ? <Check className="size-4" /> : null}
            </button>
          ))}
          <div className="my-1 border-t border-line" />
          <button
            type="button"
            onClick={() => {
              setOpen(false);
              onConnect();
            }}
            className={`${item} text-muted hover:text-fg`}
          >
            <Plus className="size-4" />
            <span className="flex-1">{t('home.connectHarness')}</span>
          </button>
        </div>
      ) : null}
    </div>
  );
}
