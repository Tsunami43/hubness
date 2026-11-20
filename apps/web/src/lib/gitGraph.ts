import type { GitCommit } from '@hubness/shared';

/**
 * Commit graph layout (lanes) for the History list. Commits come in topological order;
 * each lane "waits" for a commit hash. A row knows the lanes above and below its node:
 * that is enough to draw it as a fixed-height SVG strip.
 */

export interface GraphRow {
  /** Lane of this commit's node. */
  col: number;
  /** Lanes entering the row from above (hash each lane waits for, or null). */
  above: (string | null)[];
  /** Lanes leaving the row downwards. */
  below: (string | null)[];
  /** Lanes (from `below`) that the node connects to: its parents. */
  parentCols: number[];
  /** Lanes from `above` that merge into this node (several children). */
  childCols: number[];
  merge: boolean;
  /** Nothing above leads here: a branch tip (no line enters from the top). */
  tip: boolean;
}

export function layoutGraph(commits: GitCommit[]): { rows: GraphRow[]; width: number } {
  let lanes: (string | null)[] = [];
  let width = 1;
  const rows = commits.map((c) => {
    let col = lanes.indexOf(c.hash);
    const tip = col < 0;
    if (tip) {
      col = lanes.indexOf(null);
      if (col < 0) col = lanes.length;
      lanes[col] = c.hash;
    }
    const above = lanes.slice();
    const waiting = above.flatMap((h, i) => (h === c.hash ? [i] : []));
    const childCols = tip ? [] : waiting;
    // children converge into the node: their lanes end here
    for (const i of waiting) lanes[i] = null;
    const parentCols: number[] = [];
    c.parents.forEach((p, n) => {
      const existing = lanes.indexOf(p);
      if (existing >= 0) {
        parentCols.push(existing);
        return;
      }
      // first parent continues the commit's own lane, others take a free one
      let slot = n === 0 && lanes[col] === null ? col : lanes.indexOf(null);
      if (slot < 0) slot = lanes.length;
      lanes[slot] = p;
      parentCols.push(slot);
    });
    while (lanes.length && lanes[lanes.length - 1] === null) lanes.pop();
    const below = lanes.slice();
    width = Math.max(width, above.length, below.length);
    lanes = below.slice();
    return { col, above, below, parentCols, childCols, merge: c.parents.length > 1, tip };
  });
  return { rows, width };
}

/** Lane colors: theme syntax/status variables, so every theme keeps its own palette. */
export const LANE_COLORS = [
  'var(--color-accent)',
  'var(--syntax-entity, var(--color-ok))',
  'var(--syntax-constant, var(--color-warn))',
  'var(--syntax-keyword, var(--color-danger))',
  'var(--syntax-function, var(--color-fg))',
  'var(--syntax-tag, var(--color-ok))',
  'var(--color-muted)',
];
export const laneColor = (i: number) => LANE_COLORS[i % LANE_COLORS.length];
