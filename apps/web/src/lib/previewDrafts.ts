import type { PreviewLog } from '@hubness/shared';

/**
 * Messages for the agent drafted from the Preview panel. Pure (the translated lines come in
 * as arguments). What goes in comes from the previewed page and is not to be trusted: it is
 * cut to a size, kept inside code it can't close, and announced as page content (`note`) so
 * the agent doesn't take a line of it for an instruction.
 */

/** What the preview helper (packages/shared/assets/preview-inject.js) posts for a picked element. */
export interface PickedElement {
  selector: string;
  text: string;
  html: string;
  rect: { width: number; height: number };
  viewport: { width: number; height: number };
  path: string;
}

const MAX_HTML = 1500;
const MAX_LOG_LINES = 60;
const MAX_LOG_CHARS = 15_000;

/** A code block around `body`: the fence is longer than any run of backticks inside. */
export function fenced(body: string, lang = ''): string {
  const longest = Math.max(0, ...(body.match(/`+/g) ?? []).map((r) => r.length));
  const fence = '`'.repeat(Math.max(3, longest + 1));
  return `${fence}${lang}\n${body}\n${fence}`;
}

/** Inline code: backticks inside would end it. */
const inline = (s: string) => `\`${s.replace(/`/g, "'").replace(/\s+/g, ' ')}\``;

/** A message about a picked element; `intro` and `note` are translated lines (see above). */
export function pickDraft(p: PickedElement, intro: string, note: string): string {
  const lines = [intro, note, `- selector: ${inline(String(p.selector).slice(0, 500))}`];
  const text = String(p.text ?? '').replace(/\s+/g, ' ').trim();
  if (text) lines.push(`- text: ${inline(text.slice(0, 200))}`);
  lines.push(`- size: ${Number(p.rect?.width) || 0}×${Number(p.rect?.height) || 0}`, fenced(String(p.html ?? '').slice(0, MAX_HTML), 'html'));
  return lines.join('\n') + '\n\n';
}

/**
 * A message with what the page reported: errors, warnings and failed requests (plain logs
 * only when there is nothing else), the newest last, cut from the old end.
 */
export function logsDraft(logs: Pick<PreviewLog, 'level' | 'text'>[], intro: string, note: string): string {
  const loud = logs.filter((l) => l.level !== 'log');
  const body = (loud.length ? loud : logs)
    .slice(-MAX_LOG_LINES)
    .map((l) => `[${l.level}] ${l.text}`)
    .join('\n')
    .slice(-MAX_LOG_CHARS);
  return `${intro}\n${note}\n${fenced(body)}\n\n`;
}
