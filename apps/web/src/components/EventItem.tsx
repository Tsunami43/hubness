import { useState, type ReactNode } from 'react';
import {
  Bot,
  Check,
  ChevronRight,
  CircleAlert,
  FilePen,
  FileText,
  Globe,
  ListTodo,
  Loader2,
  Search,
  ShieldCheck,
  ShieldX,
  PanelRight,
  Terminal,
  Wrench,
  X, Folder } from 'lucide-react';
import type { HubEvent } from '@hubness/shared';
import { fmtDuration, shortModel } from '../lib/format';
import { DiffView, editToDiff } from './DiffView';
import { Markdown } from './Markdown';
import { CodeFile } from './Code';
import { t } from '../i18n';
import { MessageAttachments } from '../lib/attachments';

type Ev<K extends HubEvent['kind']> = Extract<HubEvent, { kind: K }>;
type Input = Record<string, any>;

/** Tools shown only as a request (a tool card would duplicate it). */
export const HIDDEN_TOOLS = new Set(['AskUserQuestion', 'ExitPlanMode']);

export function relPath(p: unknown, root?: string): string {
  const s = String(p ?? '');
  if (root && s.startsWith(root + '/')) return s.slice(root.length + 1);
  return s;
}

function toolMeta(name: string, input: Input, root?: string): { icon: ReactNode; title: string; detail: string } {
  const ic = 'size-4 shrink-0';
  switch (name) {
    case 'Bash':
      return { icon: <Terminal className={ic} />, title: input.description || t('tool.command'), detail: String(input.command ?? '').split('\n')[0] };
    case 'Read':
      return { icon: <FileText className={ic} />, title: t('tool.read'), detail: relPath(input.file_path, root) };
    case 'Write':
      return { icon: <FilePen className={ic} />, title: t('tool.write'), detail: relPath(input.file_path, root) };
    case 'Edit':
    case 'MultiEdit':
    case 'NotebookEdit':
      return { icon: <FilePen className={ic} />, title: t('tool.edit'), detail: relPath(input.file_path ?? input.notebook_path, root) };
    case 'Grep':
      return { icon: <Search className={ic} />, title: t('tool.search'), detail: `${input.pattern ?? ''}${input.path ? ` ${t('tool.in')} ${relPath(input.path, root)}` : ''}` };
    case 'Glob':
      return { icon: <Search className={ic} />, title: t('tool.glob'), detail: String(input.pattern ?? '') };
    case 'WebFetch':
      return { icon: <Globe className={ic} />, title: t('tool.fetch'), detail: String(input.url ?? '') };
    case 'LS':
      return { icon: <Folder className={ic} />, title: t('tool.ls'), detail: relPath(input.path, root) };
    case 'ToolSearch':
      return { icon: <Search className={ic} />, title: t('tool.toolSearch'), detail: String(input.query ?? '') };
    case 'WebSearch':
      return { icon: <Globe className={ic} />, title: t('tool.webSearch'), detail: String(input.query ?? '') };
    case 'TodoWrite': {
      const todos: Input[] = Array.isArray(input.todos) ? input.todos : [];
      const done = todos.filter((t) => t.status === 'completed').length;
      return { icon: <ListTodo className={ic} />, title: t('tool.todos'), detail: t('tool.todosDone', { done, total: todos.length }) };
    }
    case 'Task':
    case 'Agent':
      return { icon: <Bot className={ic} />, title: t('tool.subagent'), detail: String(input.description ?? '') };
    case 'mcp__hub__open_pane':
      return { icon: <PanelRight className={ic} />, title: t('tool.openPane'), detail: String(input.title ?? input.target ?? '') };
    case 'mcp__hub__close_pane':
      return { icon: <PanelRight className={ic} />, title: t('tool.closePane'), detail: '' };
    default: {
      const first = Object.values(input ?? {}).find((v) => typeof v === 'string') as string | undefined;
      const mcp = name.match(/^mcp__(.+?)__(.+)$/);
      if (mcp) return { icon: <Wrench className={ic} />, title: `${mcp[1]} · ${mcp[2].replace(/_/g, ' ')}`, detail: first ?? '' };
      return { icon: <Wrench className={ic} />, title: name, detail: first ?? '' };
    }
  }
}

function Pre({ children, className = '' }: { children: ReactNode; className?: string }) {
  return (
    <pre className={`max-h-80 overflow-auto border border-line bg-surface px-3 py-2 font-mono text-[12px] leading-relaxed whitespace-pre-wrap break-all ${className}`}>
      {children}
    </pre>
  );
}

const filePathOf = (input: Input): string | undefined => {
  const p = input.file_path ?? input.path ?? input.filePath;
  return typeof p === 'string' ? p : undefined;
};

/** Tool input details (command, diff, file contents, work plan). */
export function ToolInputDetails({ name, input }: { name: string; input: Input }) {
  if (name === 'Bash') return <Pre>$ {input.command}</Pre>;
  if (name === 'Edit')
    return (
      <div className="overflow-hidden border border-line">
        <DiffView diff={editToDiff(String(input.old_string ?? ''), String(input.new_string ?? ''))} maxHeight="20rem" path={filePathOf(input)} />
      </div>
    );
  if (name === 'MultiEdit' && Array.isArray(input.edits))
    return (
      <div className="overflow-hidden border border-line">
        <DiffView diff={input.edits.map((e: Input) => editToDiff(String(e.old_string ?? ''), String(e.new_string ?? ''))).join('\n')} maxHeight="20rem" path={filePathOf(input)} />
      </div>
    );
  if (name === 'Write')
    return (
      <div className="md max-h-80 overflow-auto rounded-lg text-[12px]">
        <CodeFile code={String(input.content ?? '')} path={filePathOf(input)} />
      </div>
    );
  if (name === 'TodoWrite' && Array.isArray(input.todos))
    return (
      <ul className="space-y-1 text-sm">
        {input.todos.map((t: Input, i: number) => (
          <li key={i} className="flex items-start gap-2">
            <span className={`mt-0.5 inline-flex size-4 shrink-0 items-center justify-center rounded border ${t.status === 'completed' ? 'border-ok bg-ok/20 text-ok' : 'border-line'}`}>
              {t.status === 'completed' ? <Check className="size-3" /> : t.status === 'in_progress' ? <span className="size-1.5 rounded-full bg-ok" /> : null}
            </span>
            <span className={t.status === 'completed' ? 'text-muted line-through' : t.status === 'in_progress' ? 'font-medium' : ''}>
              {t.content}
            </span>
          </li>
        ))}
      </ul>
    );
  return <Pre>{JSON.stringify(input, null, 2)}</Pre>;
}

function ToolCard({ ev, root }: { ev: Ev<'tool'>; root?: string }) {
  const [open, setOpen] = useState(ev.name === 'TodoWrite');
  const input = (ev.input ?? {}) as Input;
  const meta = toolMeta(ev.name, input, root);
  const statusIcon =
    ev.status === 'running' ? (
      <Loader2 className="size-4 animate-spin text-ok" />
    ) : ev.status === 'ok' ? (
      <Check className="size-4 text-ok" />
    ) : ev.status === 'denied' ? (
      <ShieldX className="size-4 text-warn" />
    ) : (
      <X className="size-4 text-danger" />
    );
  // as in opencode: a line on a thin vertical rule, details on click
  return (
    <div className={`border-l-2 ${open ? 'border-muted' : 'border-line'}`}>
      <button type="button" onClick={() => setOpen(!open)} className="flex w-full items-center gap-2 py-1 pr-1 pl-2.5 text-left hover:bg-surface">
        <span className="shrink-0 text-muted [&_svg]:size-3.5">{meta.icon}</span>
        <span className="flex min-w-0 flex-1 items-baseline gap-2 text-[13px]">
          <span className="shrink-0 text-fg">{meta.title}</span>
          {meta.detail ? <span className="min-w-0 truncate text-muted">{meta.detail}</span> : null}
        </span>
        <span className="shrink-0 [&_svg]:size-3.5">{statusIcon}</span>
        <ChevronRight className={`size-3.5 shrink-0 text-muted transition ${open ? 'rotate-90' : ''}`} />
      </button>
      {open ? (
        <div className="space-y-2 py-1.5 pr-1 pl-2.5">
          <ToolInputDetails name={ev.name} input={input} />
          {ev.output && ev.name !== 'TodoWrite' ? (
            <Pre className={ev.status === 'error' ? 'text-danger' : ev.status === 'denied' ? 'text-warn' : ''}>{ev.output}</Pre>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function ResolvedApproval({ ev }: { ev: Ev<'approval'> }) {
  if (ev.status === 'pending') return null;
  if (ev.questions && ev.answers) {
    return (
      <div className="space-y-1 border-l-2 border-line px-3 py-1.5 text-sm">
        {ev.questions.map((q) => (
          <div key={q.question}>
            <span className="text-muted">{q.question}</span> — <span className="font-medium">{ev.answers?.[q.question] ?? '—'}</span>
          </div>
        ))}
      </div>
    );
  }
  const label =
    ev.status === 'allowed'
      ? ev.toolName === 'ExitPlanMode'
        ? t('ev.planApproved')
        : t('ev.allowed', { tool: ev.toolName })
      : ev.status === 'denied'
        ? ev.reason
          ? t('ev.deniedReason', { tool: ev.toolName, reason: ev.reason })
          : t('ev.denied', { tool: ev.toolName })
        : t('ev.expired', { tool: ev.toolName });
  const Icon = ev.status === 'allowed' ? ShieldCheck : ShieldX;
  return (
    <div className={`flex items-center gap-2 pl-3 text-xs ${ev.status === 'allowed' ? 'text-muted' : 'text-warn'}`}>
      <Icon className="size-3.5" />
      {label}
    </div>
  );
}

export function EventItem({ ev, root, sessionId }: { ev: HubEvent; root?: string; sessionId?: string }) {
  switch (ev.kind) {
    case 'user':
      return (
        <div className="border-l-2 border-accent bg-surface px-3 py-2 whitespace-pre-wrap break-words">
          {ev.text}
          {ev.attachments?.length && sessionId ? <MessageAttachments sessionId={sessionId} list={ev.attachments} /> : null}
        </div>
      );
    case 'text':
      if (!ev.text && ev.streaming) return <Loader2 className="size-4 animate-spin text-muted" />;
      return (
        <div>
          <Markdown text={ev.text} breaks />
          {ev.streaming ? <span className="ml-0.5 inline-block h-4 w-1.5 animate-pulse bg-muted align-middle" /> : null}
        </div>
      );
    case 'thinking':
      return (
        <details className="group text-[13px] text-muted">
          <summary className="flex cursor-pointer list-none items-center gap-1 pl-3 italic select-none hover:text-fg [&::-webkit-details-marker]:hidden">
            <ChevronRight className="size-3.5 transition group-open:rotate-90" />
            {t('ev.thinking')}
          </summary>
          <div className="mt-1 border-l-2 border-line pl-3 whitespace-pre-wrap italic">{ev.text}</div>
        </details>
      );
    case 'tool':
      return HIDDEN_TOOLS.has(ev.name) ? null : <ToolCard ev={ev} root={root} />;
    case 'approval':
      return <ResolvedApproval ev={ev} />;
    case 'notice':
      return ev.level === 'error' ? (
        <div className="flex gap-2 border-l-2 border-danger bg-danger/5 px-3 py-2 text-sm text-danger">
          <CircleAlert className="mt-0.5 size-4 shrink-0" />
          <div className="min-w-0 font-mono text-[12px] whitespace-pre-wrap break-words">{ev.text}</div>
        </div>
      ) : (
        <div className="text-center text-xs text-muted">{ev.text}</div>
      );
    case 'result':
      return (
        <div className="flex items-center gap-2 text-xs text-muted">
          <span className="h-px flex-1 bg-line" />
          {ev.ok ? t('ev.done') : ev.stopped ? t('ev.stopped') : t('ev.failed')}
          {ev.durationMs !== undefined ? ` · ${fmtDuration(ev.durationMs)}` : ''}
          {ev.billing === 'subscription'
            ? ` · ${
                ev.limit
                  ? t('ev.subscriptionLimit', {
                      pct: Math.round(ev.limit.utilization * 100),
                      window: t(ev.limit.window === 'five_hour' ? 'ev.window5h' : 'ev.windowWeek'),
                    })
                  : t('ev.subscription')
              }`
            : ev.billing === 'api' && ev.costUsd !== undefined
              ? ` · $${ev.costUsd.toFixed(4)}`
              : ''}
          {ev.model ? ` · ${shortModel(ev.model)}` : ''}
          <span className="h-px flex-1 bg-line" />
        </div>
      );
  }
}
