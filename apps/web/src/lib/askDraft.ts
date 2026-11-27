import type { AskTarget } from '@hubness/shared';
import { t } from '../i18n';

/**
 * "Ask" on the System page: nothing is started on its own. The start page opens with the
 * input already filled in (the object's name, or the logs) and the system project `home`
 * picked — its AGENTS.md carries the server-safety rules. The user writes the question,
 * picks the agent and sends.
 */

let pending: string | null = null;
let pendingProject: string | null = null;

/** Hand a draft to the start page (picked up on its next mount); `projectId` picks that project instead of `home`. */
export function setAskDraft(text: string, projectId?: string) {
  pending = text;
  pendingProject = projectId ?? null;
}

export function takeAskDraft(): string | null {
  const d = pending;
  pending = null;
  return d;
}

/** The project the last draft was aimed at (read once, with the draft). */
export function takeAskProject(): string | null {
  const p = pendingProject;
  pendingProject = null;
  return p;
}

/** The opening line about an object: "Service `x` (system): ". */
export function askAbout(target: AskTarget): string {
  switch (target.kind) {
    case 'unit':
      return t('ask.aboutUnit', { name: target.id, scope: target.scope });
    case 'container':
      return t('ask.aboutContainer', { name: target.id });
    case 'package':
      return t('ask.aboutPackage', { name: target.id });
  }
}

const MAX_LOG_LINES = 200;
const MAX_LOG_CHARS = 20_000;

/** Logs to ask about: the tail (the newest lines matter most), in a code block. */
export function askAboutLogs(subject: string, logs: string): string {
  let tail = logs.trimEnd().split('\n').slice(-MAX_LOG_LINES).join('\n');
  if (tail.length > MAX_LOG_CHARS) tail = tail.slice(-MAX_LOG_CHARS);
  return `${t('ask.aboutLogs', { name: subject })}\n\`\`\`\n${tail}\n\`\`\`\n\n`;
}
