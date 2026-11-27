import type { PreviewLog } from '@hubness/shared';

/**
 * What the Preview console knows of a page's reports: the entries shown, how far the server's
 * log was read (`since`, a seq) and how many errors there were in all and at the last look —
 * counted, not derived from the list, so trimming old entries never hides a new error.
 * Pure: the panel keeps it in state and a ref, the tests drive it directly.
 */
export interface LogState {
  logs: PreviewLog[];
  /** The seq of the newest entry read: the next poll asks for what came after. */
  since: number;
  /** Errors and failed requests received so far. */
  errors: number;
  /** `errors` when the console was last looked at. */
  seen: number;
}

export const MAX_SHOWN = 500;

export const emptyLogs = (): LogState => ({ logs: [], since: 0, errors: 0, seen: 0 });

export const isError = (l: Pick<PreviewLog, 'level'>) => l.level === 'error' || l.level === 'network';

/**
 * Add the answer of a poll. Entries at or before `since` are dropped: two polls that overlap
 * (a slow answer) bring the same entries twice.
 */
export function mergeLogs(state: LogState, incoming: PreviewLog[], cap = MAX_SHOWN): LogState {
  const fresh = incoming.filter((l) => l.seq > state.since).sort((a, b) => a.seq - b.seq);
  if (!fresh.length) return state;
  return {
    logs: [...state.logs, ...fresh].slice(-cap),
    since: fresh[fresh.length - 1]!.seq,
    errors: state.errors + fresh.filter(isError).length,
    seen: state.seen,
  };
}

/** The console was looked at (opened, or closed after reading). */
export const markSeen = (state: LogState): LogState => (state.seen === state.errors ? state : { ...state, seen: state.errors });

/** Errors that arrived since the last look; none while the console is open. */
export const unseenErrors = (state: LogState, open: boolean) => (open ? 0 : Math.max(0, state.errors - state.seen));
