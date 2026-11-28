import type { Session } from '@hubness/shared';
import { t } from '../i18n';
import { confirmDialog } from './Confirm';

/** A regular chat (not in a project or a group) has a folder of its own that deleting it takes along. */
export const hasChatFolder = (s: Pick<Session, 'projectId' | 'groupId'>) => !s.projectId && !s.groupId;

/**
 * Ask before deleting a session. Deleting removes everything: a regular chat's folder (what the
 * agent made there) goes too — the server never touches project or group folders. null — cancelled.
 */
export async function askDeleteSession(s: Session, message: string): Promise<{ folder: boolean } | null> {
  const ok = await confirmDialog({
    message: hasChatFolder(s) ? `${message}\n\n${t('session.deleteFolderHint')}` : message,
    confirmLabel: t('common.delete'),
    danger: true,
  });
  return ok ? { folder: true } : null;
}

/** Ask before emptying the archive; regular chats' folders go along. null — cancelled. */
export async function askEmptyArchive(all: Session[]): Promise<{ folders: boolean } | null> {
  const message = t('archive.emptyConfirm', { n: all.length });
  const ok = await confirmDialog({
    title: t('archive.emptyTitle'),
    message: all.some(hasChatFolder) ? `${message}\n\n${t('archive.emptyFoldersHint')}` : message,
    confirmLabel: t('archive.empty'),
    danger: true,
  });
  return ok ? { folders: true } : null;
}
