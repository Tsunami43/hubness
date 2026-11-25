import { lazy, Suspense, useEffect, useState } from 'react';
import { Check, Copy, ExternalLink, Square } from 'lucide-react';
import type { JobInfo } from '@hubness/shared';
import { api } from '../lib/api';
import { t } from '../i18n';
import { Button, Sheet, Spinner } from './ui';

export type Status = { running: boolean; code: number | null };

export interface JobTerminalProps {
  job: JobInfo;
  className?: string;
  /** Stretch to the parent's full height (tool panel). */
  fill?: boolean;
  onStatus?: (s: Status) => void;
  /** All output without ANSI (for finding links) and raw. */
  onText?: (plain: string, raw: string) => void;
  onExit?: () => void;
  mobileKeys?: boolean;
}

const XTermJob = lazy(() => import('./XTermJob'));

/** Task terminal (xterm.js, loaded on first use). */
export function JobTerminal(props: JobTerminalProps) {
  const fallback = (
    <div className={`flex items-center justify-center border border-line bg-surface ${props.fill ? 'min-h-0 flex-1' : (props.className ?? 'h-[46dvh] sm:h-[420px]')}`}>
      <Spinner />
    </div>
  );
  return (
    <Suspense fallback={fallback}>
      <XTermJob {...props} />
    </Suspense>
  );
}

const URL_RE = /https?:\/\/[^\s\x1b\x07"'<>]+/g;

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

/** pty size for a terminal in TerminalSheet: the sheet width in characters (xterm: ~7.9px each). */
export function sheetTermSize(rows: number) {
  const w = Math.min(window.innerWidth, 768) - 56;
  return { cols: Math.max(40, Math.floor(w / 7.9)), rows };
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
