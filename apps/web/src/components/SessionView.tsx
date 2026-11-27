import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { useNavigate } from 'react-router';
import { Archive, ArchiveRestore, ArrowUp, FolderInput, Loader2, Pencil, RotateCcw, Settings2, Square, Trash2 } from 'lucide-react';
import type { AgentConfig, HubEvent, ProjectSuggestion } from '@hubness/shared';
import { api } from '../lib/api';
import { confirmDialog } from './Confirm';
import { agentLabel, permissionLabel } from '../lib/format';
import { useLocalStorage, useSessionStream, type SessionScope } from '../lib/hooks';
import { AgentPicker } from './AgentPicker';
import { useAgents, validAgent } from '../lib/agents';
import { ApprovalCard } from './ApprovalCard';
import { EventItem } from './EventItem';
import { AgentIcon } from './AgentIcon';
import { PaneView } from './PaneView';
import { Button, ErrorLine, Field, IconButton, inputCls, PanelHeader, Sheet, Spinner, StatusBadge, TopBar } from './ui';
import { t } from '../i18n';
import { pushDraft, useDraft } from '../lib/ask';
import { AttachButton, createWithFiles, PendingFiles, sendWithFiles, usePendingFiles, type PendingFile } from '../lib/attachments';
import { MicButton } from '../lib/voice';
import { useHubSettings } from '../lib/hubSettings';

const isTouch = () => window.matchMedia('(pointer: coarse)').matches;

/**
 * Input box, as in Claude/ChatGPT: a floating card above the feed (with
 * bottom spacing and a gradient), a multiline field, and a bar below: agent and
 * mode (open settings), status, send/stop. Enter sends,
 * Shift+Enter inserts a new line (on phone Enter inserts a new line).
 */
function Composer({
  onSend,
  onStop,
  busy,
  disabled,
  placeholder,
  toolbar,
  minRows = 2,
  autoFocus,
  draftKey,
}: {
  /** Send the text and attached files; true — sent (the input is cleared). */
  onSend: (text: string, files: PendingFile[]) => Promise<boolean>;
  onStop?: () => void;
  busy: boolean;
  disabled?: boolean;
  placeholder: string;
  /** Left side of the bar (agent, mode). */
  toolbar?: ReactNode;
  minRows?: number;
  autoFocus?: boolean;
  /** Drafts key ("ask the agent" about selected code/output). */
  draftKey?: string;
}) {
  const [text, setText] = useState('');
  const [sending, setSending] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);
  const att = usePendingFiles();
  const ready = Boolean(text.trim() || att.files.length);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, Math.round((window.visualViewport?.height ?? window.innerHeight) * 0.4))}px`;
  }, [text]);

  useEffect(() => {
    if (autoFocus && !isTouch()) ref.current?.focus();
  }, [autoFocus]);

  useDraft(draftKey, (d) => {
    setText((prev) => (prev.trim() ? `${prev.trimEnd()}\n\n${d}` : d));
    setTimeout(() => {
      const el = ref.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
    }, 0);
  });

  /** A transcript goes after the text already typed; the caret moves to its end. */
  const insertVoice = (spoken: string) => {
    setText((prev) => (prev.trim() ? `${prev.trimEnd()} ${spoken}` : spoken));
    setTimeout(() => {
      const el = ref.current;
      if (!el) return;
      if (!isTouch()) el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
    }, 0);
  };

  const send = async () => {
    if (!ready || sending || busy || disabled) return;
    setSending(true);
    const ok = await onSend(text.trim(), att.files);
    setSending(false);
    if (ok) {
      setText('');
      att.clear();
    }
  };

  // TUI (like opencode): a frame with ">", below it a status line: agent, mode, keys
  return (
    <div className="shrink-0 px-2 pb-[max(0.5rem,var(--safe-b))] sm:px-4">
      <div className="mx-auto mb-2 max-w-3xl sm:mb-3">
        <div
          {...att.dropProps}
          className={`border bg-surface py-2 pr-2 pl-3 transition focus-within:border-accent ${att.dragging ? 'border-accent bg-accent/5' : 'border-line'}`}
        >
        <PendingFiles files={att.files} onRemove={att.remove} />
        <div
          className="flex cursor-text items-start gap-2"
          onClick={(e) => {
            if (e.target === e.currentTarget) ref.current?.focus();
          }}
        >
          <span className="pt-px leading-6 text-accent select-none">&gt;</span>
          <textarea
            ref={ref}
            data-autofocus
            rows={minRows}
            value={text}
            placeholder={att.dragging ? t('attach.drop') : placeholder}
            onPaste={att.onPaste}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape' && busy && onStop) {
                e.preventDefault();
                onStop();
                return;
              }
              if (e.key === 'Enter' && (e.metaKey || e.ctrlKey || (!e.shiftKey && !isTouch()))) {
                e.preventDefault();
                void send();
              }
            }}
            className="min-w-0 flex-1 resize-none [scrollbar-width:thin] bg-transparent text-[15px] leading-6 outline-none placeholder:text-muted/70 sm:text-[14px]"
          />
          <MicButton onText={insertVoice} onError={att.setError} />
          <AttachButton onPick={att.add} />
          {busy && onStop ? (
            <IconButton label={t('session.stop')} onClick={onStop} className="size-8 border border-danger/60 text-danger hover:bg-danger/10 hover:text-danger">
              <Square className="size-3 fill-current" />
            </IconButton>
          ) : (
            // not an IconButton: its muted text and faded disabled state would wash out the accent fill
            <button
              type="button"
              aria-label={t('session.send')}
              title={t('session.send')}
              onClick={() => void send()}
              disabled={!ready || sending || disabled}
              className={`tap inline-flex size-8 shrink-0 items-center justify-center ${ready || sending ? 'btn-accent' : 'border border-line text-muted'}`}
            >
              {sending ? <Loader2 className="size-4 animate-spin" /> : <ArrowUp className="size-4" strokeWidth={2.5} />}
            </button>
          )}
        </div>
        </div>
        <ErrorLine error={att.error} />
        <div className="flex min-h-6 items-center gap-3 px-1 pt-1 text-xs text-muted">
          {toolbar}
          {busy ? (
            <span className="inline-flex min-w-0 items-center gap-1.5 text-ok">
              <Loader2 className="size-3 shrink-0 animate-spin" />
              <span className="truncate">{t('session.working')}</span>
            </span>
          ) : null}
          {!isTouch() ? (
            <span className="ml-auto hidden shrink-0 md:inline">
              {busy && onStop ? (
                <>
                  <span className="text-fg">esc</span> {t('session.keyStop')}
                </>
              ) : (
                <>
                  <span className="text-fg">enter</span> {t('session.keySend')} · <span className="text-fg">shift+enter</span> {t('session.keyNewline')}
                </>
              )}
            </span>
          ) : null}
        </div>
      </div>
    </div>
  );
}

/** Agent chip in the input box: icon, name, mode; opens the session settings. */
function AgentChip({ agent, onClick }: { agent: AgentConfig; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} title={t('session.settings')} className="inline-flex min-w-0 items-center gap-1.5 hover:text-fg">
      <AgentIcon kind={agent.kind} className="size-3 shrink-0" />
      <span className="truncate text-accent">{agentLabel(agent)}</span>
      <span className="shrink-0">· {permissionLabel(agent.permissionMode ?? 'ask')}</span>
    </button>
  );
}

/** New session: agent picker + first message. */
export function NewSessionView({
  scope,
  backTo,
  basePath,
  title,
  onBack,
  onCreated,
  draftKey,
}: {
  draftKey?: string;
  /** Project or group; null means a plain chat. */
  scope: SessionScope | null;
  backTo: string;
  basePath: string;
  title: string;
  /** Embedded mode (project tab): compact header and "back" without navigation. */
  onBack?: () => void;
  onCreated?: (sessionId: string) => void;
}) {
  const nav = useNavigate();
  const [stored, setAgent] = useLocalStorage<AgentConfig | null>(scope ? 'hub.agent.project' : 'hub.agent.chat', null);
  // the remembered agent if it is still registered, otherwise the hub default
  const agent = validAgent(stored, useAgents());
  // a new session starts with the default permissions (Settings → Agent); harness and model are remembered
  const defaultPermission = useHubSettings()?.defaultPermission;
  useEffect(() => {
    if (agent && defaultPermission && agent.permissionMode !== defaultPermission) setAgent({ ...agent, permissionMode: defaultPermission });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [defaultPermission, agent?.kind]);
  const [error, setError] = useState<string | null>(null);

  const send = async (prompt: string, files: PendingFile[]) => {
    if (!agent) return false;
    try {
      const s = await createWithFiles({ ...(scope ?? {}), agent }, prompt, files);
      if (onCreated) onCreated(s.id);
      else nav(`${basePath}/${s.id}`, { replace: true });
      return true;
    } catch (e) {
      setError((e as Error).message);
      return false;
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      {onBack ? <PanelHeader title={title} onBack={onBack} /> : <TopBar title={title} back={backTo} />}
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-6">
        <div className="mx-auto max-w-xl space-y-4">
          <div className="text-sm text-muted">{t('agent.who')}</div>
          {agent ? <AgentPicker value={agent} onChange={setAgent} /> : <Spinner />}
          <ErrorLine error={error} />
        </div>
      </div>
      <Composer
        onSend={send}
        busy={false}
        minRows={3}
        autoFocus
        draftKey={draftKey}
        placeholder={scope ? t('session.placeholderProject') : t('session.placeholderNew')}
      />
    </div>
  );
}

/** Inbox chat where code appeared: offer to move it into a project. */
function ConvertBanner({ sessionId, status }: { sessionId: string; status: string }) {
  const nav = useNavigate();
  const [sugg, setSugg] = useState<ProjectSuggestion | null>(null);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('');
  const root = (useHubSettings()?.projectsRoot ?? '~/hub/projects').replace(/\/+$/, '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dismissed, setDismissed] = useState(false);

  // check after every agent turn
  useEffect(() => {
    if (status !== 'idle') return;
    api.projectSuggestion(sessionId).then((r) => {
      setSugg(r);
      setName(r.name);
    }, () => {});
  }, [sessionId, status]);

  if (!sugg?.suggest || dismissed) return null;
  return (
    <>
      <div className="shrink-0 border-t border-line bg-accent/8 px-4 py-2">
        <div className="mx-auto flex max-w-3xl flex-wrap items-center gap-2 text-sm">
          <FolderInput className="size-4 shrink-0 text-accent" />
          <span className="min-w-0 flex-1">
            {t('convert.banner')} <span className="text-muted">({sugg.reason})</span>
          </span>
          <Button size="sm" variant="primary" onClick={() => setOpen(true)}>
            {t('convert.action')}
          </Button>
          <button type="button" className="text-xs text-muted hover:text-fg" onClick={() => setDismissed(true)}>
            {t('convert.later')}
          </button>
        </div>
      </div>
      <Sheet
        open={open}
        onClose={() => setOpen(false)}
        title={t('convert.title')}
        footer={
          <Button
            className="flex-1"
            variant="primary"
            loading={busy}
            disabled={!name.trim()}
            onClick={async () => {
              setBusy(true);
              try {
                const r = await api.convertSession(sessionId, name);
                nav(`/projects/${r.project.id}/s/${sessionId}`, { replace: true });
              } catch (e) {
                setError((e as Error).message);
                setBusy(false);
              }
            }}
          >
            {t('convert.confirm')}
          </Button>
        }
      >
        <div className="space-y-4">
          <p className="text-sm text-muted">{t('convert.hint', { root })}</p>
          <Field label={t('common.name')} hint={t('convert.nameHint', { root, name: name.trim() || '…' })}>
            <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
          <ErrorLine error={error} />
        </div>
      </Sheet>
    </>
  );
}

/** Existing session: live feed, answers to requests, continuing the conversation, agent panel. */
export function SessionView({
  sessionId,
  backTo,
  root,
  onBack,
  draftKey: draftKeyProp,
}: {
  sessionId: string;
  backTo: string;
  root?: string;
  draftKey?: string;
  /** Embedded mode (project tab): compact header, agent panel as a tab. */
  onBack?: () => void;
}) {
  // the input always has a drafts key ("Edit" on a failed message puts the text there)
  const draftKey = draftKeyProp ?? `session:${sessionId}`;
  // in embedded mode the panel is narrow, so use the phone layout (no lg split)
  const wide = !onBack;
  const nav = useNavigate();
  const { session, setSession, events, pane, setPane } = useSessionStream(sessionId);
  const [error, setError] = useState<string | null>(null);
  const [settings, setSettings] = useState(false);
  const [mobileTab, setMobileTab] = useState<'chat' | 'pane'>('chat');
  const scrollRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  // Autoscroll to the bottom if the user was already at the bottom
  useEffect(() => {
    const el = scrollRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [events, mobileTab]);

  // the feed got shorter (on-screen keyboard, a taller input box): keep the last message in view
  const loaded = Boolean(session);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      if (stick.current) el.scrollTop = el.scrollHeight;
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [loaded, mobileTab]);

  // a new panel from the agent: show it right away on phone
  useEffect(() => {
    setMobileTab(pane ? 'pane' : 'chat');
  }, [pane?.at]);
  const [retrying, setRetrying] = useState(false);

  if (!session) {
    return (
      <div className="flex h-full flex-col">
        {onBack ? <PanelHeader title="…" onBack={onBack} /> : <TopBar title="…" back={backTo} />}
        <div className="flex flex-1 items-center justify-center">
          <Spinner />
        </div>
      </div>
    );
  }

  const busy = session.status === 'running' || session.status === 'waiting';
  // the last message got no successful answer (error, Stop, hub restart, no reply): offer to resend it
  const lastUser = [...events].reverse().find((e): e is Extract<HubEvent, { kind: 'user' }> => e.kind === 'user');
  const answered = lastUser ? events.some((e) => e.kind === 'result' && e.ok && e.seq > lastUser.seq) : true;
  const retry = !busy && lastUser && !answered ? lastUser : null;
  const resend = async () => {
    if (!retry) return;
    setRetrying(true);
    setError(null);
    try {
      await api.send(sessionId, retry.text, retry.attachments);
      stick.current = true;
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setRetrying(false);
    }
  };
  const pending = events.filter((e): e is Extract<HubEvent, { kind: 'approval' }> => e.kind === 'approval' && e.status === 'pending');
  // paths in tool cards are relative to the project or chat folder
  root = root ?? session.cwd ?? undefined;

  const send = async (text: string, files: PendingFile[]) => {
    try {
      setError(null);
      await sendWithFiles(sessionId, text, files);
      stick.current = true;
      setMobileTab('chat');
      return true;
    } catch (e) {
      setError((e as Error).message);
      return false;
    }
  };

  const closePane = () => {
    setPane(null);
    void api.closePane(sessionId);
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      {onBack ? (
        <PanelHeader
          title={session.title}
          onBack={onBack}
          actions={
            <IconButton label={t('session.settings')} onClick={() => setSettings(true)} className="size-7">
              <Settings2 className="size-4" />
            </IconButton>
          }
        />
      ) : (
        <TopBar
          title={session.title}
          subtitle={
            <span className="inline-flex items-center gap-1.5">
              <AgentIcon kind={session.agent.kind} className="size-3" />
              {agentLabel(session.agent)}
              {` · ${permissionLabel(session.agent.permissionMode ?? 'ask')}`}
            </span>
          }
          back={backTo}
          actions={
            <IconButton label={t('session.settings')} onClick={() => setSettings(true)}>
              <Settings2 className="size-4" />
            </IconButton>
          }
        />
      )}
      {pane ? (
        <div className={`flex shrink-0 gap-1 border-b border-line bg-surface p-1 ${wide ? 'lg:hidden' : ''}`}>
          {(['chat', 'pane'] as const).map((tab) => (
            <button
              key={tab}
              type="button"
              onClick={() => setMobileTab(tab)}
              className={`flex-1 rounded-lg py-1.5 text-sm ${mobileTab === tab ? 'bg-sunken font-medium text-fg' : 'text-muted'}`}
            >
              {tab === 'chat' ? t('pane.chat') : pane.title || t('pane.panel')}
            </button>
          ))}
        </div>
      ) : null}
      <div className="flex min-h-0 flex-1">
        <div className={`min-h-0 min-w-0 flex-1 flex-col ${pane && mobileTab === 'pane' ? (wide ? 'hidden lg:flex' : 'hidden') : 'flex'}`}>
          <div
            ref={scrollRef}
            onScroll={(e) => {
              const el = e.currentTarget;
              stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
            }}
            className="min-h-0 flex-1 overflow-y-auto px-4 pt-4 pb-6"
          >
            <div className="mx-auto max-w-3xl space-y-2.5">
              {events.map((ev) =>
                ev.kind === 'approval' && ev.status === 'pending' ? (
                  <ApprovalCard
                    key={ev.uid}
                    ev={ev}
                    root={root}
                    onDecide={async (d) => {
                      try {
                        await api.approve(sessionId, ev.requestId, d);
                      } catch (e) {
                        setError((e as Error).message);
                      }
                    }}
                  />
                ) : (
                  <EventItem key={ev.uid} ev={ev} root={root} sessionId={sessionId} />
                ),
              )}
              {retry ? (
                <div className="flex flex-wrap items-center gap-2 text-xs text-muted">
                  <span className="min-w-0 flex-1">{t('session.noAnswer')}</span>
                  <Button size="sm" variant="ghost" onClick={() => pushDraft(draftKey, retry.text)} title={t('session.editHint')}>
                    <Pencil className="size-3.5" /> {t('session.edit')}
                  </Button>
                  <Button size="sm" loading={retrying} onClick={() => void resend()}>
                    <RotateCcw className="size-3.5" /> {t('session.retry')}
                  </Button>
                </div>
              ) : null}
              <ErrorLine error={error} />
            </div>
          </div>
          {pending.length > 0 ? (
            <div className="shrink-0 border-t border-line bg-warn/10 px-4 py-1.5 text-center text-sm text-warn">
              <StatusBadge status="waiting" /> — {t('session.answerAbove')}
            </div>
          ) : null}
          {session.archivedAt ? (
            <div className="flex shrink-0 flex-wrap items-center gap-2 border-t border-line bg-sunken px-4 py-1.5 text-sm">
              <Archive className="size-3.5 shrink-0 text-muted" />
              <span className="min-w-0 flex-1 text-muted">{t('archive.banner')}</span>
              <Button size="sm" onClick={() => api.restoreSession(sessionId).then(setSession, (e: Error) => setError(e.message))}>
                <ArchiveRestore className="size-3.5" /> {t('archive.restore')}
              </Button>
            </div>
          ) : null}
          {!session.projectId && !session.groupId ? <ConvertBanner sessionId={sessionId} status={session.status} /> : null}
          <Composer
            onSend={send}
            onStop={() => void api.stop(sessionId)}
            busy={busy}
            autoFocus
            draftKey={draftKey}
            placeholder={busy ? t('session.placeholderBusy') : t('session.placeholderReply')}
            toolbar={<AgentChip agent={session.agent} onClick={() => setSettings(true)} />}
          />
        </div>
        {pane ? (
          <div className={`min-h-0 w-full flex-col border-l border-line ${wide ? 'lg:flex lg:w-[48%]' : ''} ${mobileTab === 'pane' ? 'flex' : 'hidden'}`}>
            <PaneView sessionId={sessionId} pane={pane} onClose={closePane} />
          </div>
        ) : null}
      </div>
      <SessionSettings
        open={settings}
        onClose={() => setSettings(false)}
        session={session}
        onSaved={setSession}
        onDeleted={() => (onBack ? onBack() : nav(backTo, { replace: true }))}
      />
    </div>
  );
}

function SessionSettings({
  open,
  onClose,
  session,
  onSaved,
  onDeleted,
}: {
  open: boolean;
  onClose: () => void;
  session: NonNullable<ReturnType<typeof useSessionStream>['session']>;
  onSaved: (s: typeof session) => void;
  onDeleted: () => void;
}) {
  const [title, setTitle] = useState(session.title);
  const [agent, setAgent] = useState(session.agent);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setTitle(session.title);
      setAgent(session.agent);
      setError(null);
    }
  }, [open, session]);

  const save = async () => {
    setBusy(true);
    try {
      onSaved(await api.updateSession(session.id, { title, agent: { model: agent.model, permissionMode: agent.permissionMode } }));
      onClose();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!(await confirmDialog({ message: t('session.deleteConfirm'), confirmLabel: t('common.delete'), danger: true }))) return;
    await api.deleteSession(session.id);
    onDeleted();
  };
  // to the archive and back: the session stays open
  const toggleArchive = async () => {
    try {
      onSaved(await (session.archivedAt ? api.restoreSession(session.id) : api.archiveSession(session.id)));
      onClose();
    } catch (e) {
      setError((e as Error).message);
    }
  };

  return (
    <Sheet
      open={open}
      onClose={onClose}
      title={t('session.settingsTitle')}
      footer={
        <>
          <Button variant="danger" onClick={remove} title={t('common.delete')} aria-label={t('common.delete')}>
            <Trash2 className="size-4" />
          </Button>
          <Button
            onClick={() => void toggleArchive()}
            title={session.archivedAt ? t('archive.restore') : t('archive.move')}
            aria-label={session.archivedAt ? t('archive.restore') : t('archive.move')}
          >
            {session.archivedAt ? <ArchiveRestore className="size-4" /> : <Archive className="size-4" />}
          </Button>
          <Button className="flex-1" variant="primary" loading={busy} onClick={save}>
            {t('common.save')}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field label={t('session.title')}>
          <input className={inputCls} value={title} onChange={(e) => setTitle(e.target.value)} />
        </Field>
        <div className="rounded-lg bg-sunken px-3 py-2 text-sm text-muted">
          {t('session.agentLocked', { agent: agentLabel(session.agent) })}
        </div>
        <AgentPicker value={agent} onChange={setAgent} lockKind />
        <ErrorLine error={error} />
      </div>
    </Sheet>
  );
}
