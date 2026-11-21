import { confirmDialog } from '../../components/Confirm';
import { t } from '../../i18n';
import { type Run } from './types';


// ---------- actions shared by keys and buttons ----------

export async function discard(run: Run, paths: string[], what: string) {
  if (await confirmDialog({ message: t('git.discardConfirm', { what }), confirmLabel: t('git.discard'), danger: true })) await run('discard', t('git.discard'), { paths });
}
export async function switchBranch(run: Run, name: string) {
  await run('checkout', t('git.switching', { branch: name }), { branch: name, create: false });
}
export async function mergeBranch(run: Run, branch: string, into: string) {
  if (await confirmDialog({ message: t('git.mergeConfirm', { branch, into }), confirmLabel: 'Merge' })) await run('merge', `Merge ${branch}`, { branch });
}
export async function rebaseBranch(run: Run, branch: string, onto: string) {
  if (await confirmDialog({ message: t('git.rebaseConfirm', { branch, onto }), confirmLabel: 'Rebase' })) await run('rebase', `Rebase ${branch} → ${onto}`, { branch: onto });
}
export async function deleteBranch(run: Run, branch: string) {
  if (!(await confirmDialog({ message: t('git.deleteBranchConfirm', { branch }), confirmLabel: t('common.delete'), danger: true }))) return;
  const r = await run('delete-branch', t('git.deleteBranch'), { branch });
  if (!r.ok && /not fully merged/i.test(r.output) && (await confirmDialog({ message: t('git.deleteUnmerged', { branch }), confirmLabel: t('git.forceDelete'), danger: true })))
    await run('delete-branch', t('git.deleteBranch'), { branch, force: true });
}
export async function dropStash(run: Run, ref: string) {
  if (await confirmDialog({ message: t('git.stashDropConfirm', { ref }), confirmLabel: t('common.delete'), danger: true })) await run('stash', `Stash drop ${ref}`, { action: 'drop', rev: ref });
}
