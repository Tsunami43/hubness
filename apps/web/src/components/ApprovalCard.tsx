import { useState } from 'react';
import { MessageCircleQuestion, ShieldAlert, ClipboardList } from 'lucide-react';
import type { ApprovalDecision, HubEvent } from '@hubness/shared';
import { Button, inputCls } from './ui';
import { Markdown } from './Markdown';
import { relPath, ToolInputDetails } from './EventItem';
import { t } from '../i18n';

type Approval = Extract<HubEvent, { kind: 'approval' }>;

/** A pending agent request: permission, question or plan. */
export function ApprovalCard({
  ev,
  root,
  onDecide,
}: {
  ev: Approval;
  root?: string;
  onDecide: (d: ApprovalDecision) => Promise<void>;
}) {
  const [busy, setBusy] = useState<'allow' | 'deny' | null>(null);
  const [denying, setDenying] = useState(false);
  const [reason, setReason] = useState('');
  const input = (ev.input ?? {}) as Record<string, any>;

  const decide = async (d: ApprovalDecision) => {
    setBusy(d.decision);
    try {
      await onDecide(d);
    } finally {
      setBusy(null);
    }
  };

  if (ev.questions?.length) return <QuestionsCard ev={ev} onDecide={decide} busy={busy !== null} />;

  const isPlan = ev.toolName === 'ExitPlanMode';
  const Icon = isPlan ? ClipboardList : ShieldAlert;
  const path = input.file_path ? relPath(input.file_path, root) : null;

  return (
    <div className="rounded-xl border-2 border-warn/60 bg-surface">
      <div className="flex items-center gap-2 px-3.5 pt-3 font-medium">
        <Icon className="size-5 text-warn" />
        {isPlan ? t('ap.plan') : t('ap.allowTool', { tool: ev.toolName })}
      </div>
      <div className="space-y-2 px-3.5 py-2.5">
        {isPlan ? (
          <div className="max-h-[50dvh] overflow-y-auto rounded-lg bg-sunken px-3 py-2">
            <Markdown text={String(input.plan ?? '')} breaks />
          </div>
        ) : (
          <>
            {path ? <div className="font-mono text-[13px] break-all">{path}</div> : null}
            <ToolInputDetails name={ev.toolName} input={input} />
          </>
        )}
        {denying ? (
          <textarea
            autoFocus
            rows={2}
            className={inputCls}
            placeholder={isPlan ? t('ap.reasonPlan') : t('ap.reasonTool')}
            value={reason}
            onChange={(e) => setReason(e.target.value)}
          />
        ) : null}
      </div>
      <div className="flex gap-2 px-3.5 pb-3.5">
        {denying ? (
          <>
            <Button variant="ghost" onClick={() => setDenying(false)}>
              {t('common.back')}
            </Button>
            <Button className="flex-1" variant="danger" loading={busy === 'deny'} onClick={() => decide({ decision: 'deny', reason })}>
              {isPlan ? t('ap.sendRevision') : t('ap.deny')}
            </Button>
          </>
        ) : (
          <>
            <Button className="flex-1" variant="secondary" onClick={() => setDenying(true)}>
              {isPlan ? t('ap.revise') : t('ap.deny')}
            </Button>
            <Button className="flex-1" variant="primary" loading={busy === 'allow'} onClick={() => decide({ decision: 'allow' })}>
              {isPlan ? t('ap.approve') : t('ap.allow')}
            </Button>
          </>
        )}
      </div>
    </div>
  );
}

function QuestionsCard({
  ev,
  onDecide,
  busy,
}: {
  ev: Approval;
  onDecide: (d: ApprovalDecision) => Promise<void>;
  busy: boolean;
}) {
  const qs = ev.questions ?? [];
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const [custom, setCustom] = useState<Record<string, string>>({});

  const answerOf = (q: string) => (custom[q]?.trim() ? custom[q].trim() : (picked[q] ?? []).join(', '));
  const complete = qs.every((q) => answerOf(q.question));

  const toggle = (q: string, label: string, multi?: boolean) =>
    setPicked((p) => {
      const cur = p[q] ?? [];
      const next = multi ? (cur.includes(label) ? cur.filter((x) => x !== label) : [...cur, label]) : [label];
      return { ...p, [q]: next };
    });

  return (
    <div className="rounded-xl border-2 border-warn/60 bg-surface px-3.5 py-3">
      <div className="mb-2 flex items-center gap-2 font-medium">
        <MessageCircleQuestion className="size-5 text-warn" />
        {t('ap.question')}
      </div>
      <div className="space-y-4">
        {qs.map((q) => (
          <div key={q.question}>
            <div className="mb-2">{q.question}</div>
            <div className="space-y-1.5">
              {q.options.map((o) => {
                const on = (picked[q.question] ?? []).includes(o.label) && !custom[q.question]?.trim();
                return (
                  <button
                    key={o.label}
                    type="button"
                    onClick={() => toggle(q.question, o.label, q.multiSelect)}
                    className={`block w-full rounded-lg border px-3 py-2 text-left ${on ? 'border-accent ring-1 ring-accent' : 'border-line hover:bg-sunken'}`}
                  >
                    <div className="text-sm font-medium">{o.label}</div>
                    {o.description ? <div className="text-xs text-muted">{o.description}</div> : null}
                  </button>
                );
              })}
              <input
                className={inputCls}
                placeholder={t('ap.customAnswer')}
                value={custom[q.question] ?? ''}
                onChange={(e) => setCustom({ ...custom, [q.question]: e.target.value })}
              />
            </div>
          </div>
        ))}
      </div>
      <div className="mt-3 flex gap-2">
        <Button variant="secondary" disabled={busy} onClick={() => onDecide({ decision: 'deny', reason: 'The user chose not to answer' })}>
          {t('ap.skip')}
        </Button>
        <Button
          className="flex-1"
          variant="primary"
          disabled={!complete}
          loading={busy}
          onClick={() => onDecide({ decision: 'allow', answers: Object.fromEntries(qs.map((q) => [q.question, answerOf(q.question)])) })}
        >
          {t('ap.answer')}
        </Button>
      </div>
    </div>
  );
}
