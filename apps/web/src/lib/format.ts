import type { AgentConfig, PermissionMode, SessionStatus } from '@hubness/shared';
import { getLang, t } from '../i18n';
import { agentName } from './agents';

export function timeAgo(iso: string): string {
  const sec = (Date.now() - new Date(iso).getTime()) / 1000;
  if (sec < 60) return t('time.justNow');
  const rtf = new Intl.RelativeTimeFormat(getLang(), { numeric: 'auto', style: 'short' });
  if (sec < 3600) return rtf.format(-Math.floor(sec / 60), 'minute');
  if (sec < 86400) return rtf.format(-Math.floor(sec / 3600), 'hour');
  if (sec < 86400 * 7) return rtf.format(-Math.floor(sec / 86400), 'day');
  return new Date(iso).toLocaleDateString(getLang(), { day: 'numeric', month: 'short' });
}

export const permissionLabel = (m: PermissionMode) => t(`perm.${m}`);
export const permissionHint = (m: PermissionMode) => t(`perm.${m}.hint`);
export const PERMISSION_MODES: PermissionMode[] = ['ask', 'acceptEdits', 'bypass', 'plan'];

export const statusLabel = (s: SessionStatus) => t(`status.${s}`);

export function agentLabel(a: AgentConfig): string {
  const name = agentName(a.kind);
  return a.model ? `${name} · ${shortModel(a.model)}` : name;
}

export function shortModel(m?: string): string {
  if (!m) return t('agent.defaultModel');
  return m.includes('/') ? m.split('/').pop()! : m;
}

/** "in 1 day" (localized) for a moment in the future. */
export function timeUntil(iso: string): string {
  const sec = (new Date(iso).getTime() - Date.now()) / 1000;
  const rtf = new Intl.RelativeTimeFormat(getLang(), { numeric: 'auto' });
  if (sec < 3600) return rtf.format(Math.max(1, Math.round(sec / 60)), 'minute');
  if (sec < 86400) return rtf.format(Math.round(sec / 3600), 'hour');
  return rtf.format(Math.round(sec / 86400), 'day');
}

export function fmtDuration(ms?: number): string {
  if (ms === undefined) return '';
  return ms < 60_000 ? `${(ms / 1000).toFixed(1)}s` : `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;
}

export function fmtSize(n?: number): string {
  if (n === undefined) return '';
  return n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function splitPath(p: string): { dir: string; name: string } {
  const i = p.lastIndexOf('/');
  return i < 0 ? { dir: '', name: p } : { dir: p.slice(0, i + 1), name: p.slice(i + 1) };
}
