import type { Session } from '@hubness/shared';
import { t } from '../i18n';
import { choiceDialog, confirmDialog } from './Confirm';

/** A regular chat (not in a project or a group) has a folder of its own that deleting it may take along. */
export const hasChatFolder = (s: Pick<Session, 'projectId' | 'groupId'>) => !s.projectId && !s.groupId;

/**
 * Ask before deleting a session. A regular chat offers to delete its folder too (what the
 * agent made there); the plain delete is the default. null — cancelled.
 */
export async function askDeleteSession(s: Session, message: string): Promise<{ folder: boolean } | null> {
  if (!hasChatFolder(s)) return (await confirmDialog({ message, confirmLabel: t('common.delete'), danger: true })) ? { folder: false } : null;
  const id = await choiceDialog({
    message: `${message}\n\n${t('session.deleteFolderHint')}`,
    choices: [
      { id: 'folder', label: t('session.deleteWithFolder'), variant: 'danger' },
      { id: 'chat', label: t('common.delete'), variant: 'danger' },
    ],
  });
  return id ? { folder: id === 'folder' } : null;
}

/** Ask before emptying the archive; regular chats among them offer to take their folders along. null — cancelled. */
export async function askEmptyArchive(all: Session[]): Promise<{ folders: boolean } | null> {
  const message = t('archive.emptyConfirm', { n: all.length });
  const title = t('archive.emptyTitle');
  if (!all.some(hasChatFolder)) return (await confirmDialog({ title, message, confirmLabel: t('archive.empty'), danger: true })) ? { folders: false } : null;
  const id = await choiceDialog({
    title,
    message: `${message}\n\n${t('archive.emptyFoldersHint')}`,
    choices: [
      { id: 'folders', label: t('archive.emptyWithFolders'), variant: 'danger' },
      { id: 'sessions', label: t('archive.empty'), variant: 'danger' },
    ],
  });
  return id ? { folders: id === 'folders' } : null;
}
