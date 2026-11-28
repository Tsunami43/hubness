import type { HubVersion } from '@hubness/shared';

/** How long the page waits for an update (download + restart) before it gives up. */
export const UPDATE_TIMEOUT_MS = 5 * 60 * 1000;

export type UpdateStep = 'wait' | 'reload' | 'installed' | 'failed' | 'timeout';

/**
 * What the System page does on each poll after "Update": the hub restarts with a new
 * version (reload the page for the new web app), reports the outcome, or doesn't answer
 * yet (`now` null: it is restarting).
 */
export function updateStep(startedVersion: string, now: HubVersion | null, elapsedMs: number): UpdateStep {
  if (now && now.version !== startedVersion) return 'reload';
  if (now?.update.state === 'failed') return 'failed';
  if (now?.update.state === 'installed') return 'installed';
  return elapsedMs > UPDATE_TIMEOUT_MS ? 'timeout' : 'wait';
}
