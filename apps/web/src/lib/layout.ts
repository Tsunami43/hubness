/**
 * Workbench layout (editor groups and tabs, like VS Code): pure functions over a plain
 * object, persisted per project in localStorage.
 *
 * - A tab is a file (repository + path), a tool (Sessions list, Run, Shell…) or an agent
 *   session (`new:<n>` — a draft that becomes the real id once created).
 * - Opening anything (file, tool, session) puts it into the focused group — it never jumps
 *   to a copy in another group; splitting a tool or a session moves it, a file is copied.
 * - A preview tab (single click in the tree) is replaced by the next preview; it becomes a
 *   regular tab on double click, on edit or when opened explicitly.
 * - Groups form a split tree: rows and columns nested in any way (split right / down,
 *   drop onto any edge), at most MAX_GROUPS groups.
 */

export type TabRef =
  | { kind: 'file'; repo: string; path: string }
  | { kind: 'tool'; id: string }
  | { kind: 'session'; id: string }
  /** One terminal job; `title` is its name (Shell 2). */
  | { kind: 'shell'; id: string; title: string };

/** Tools and sessions move when split (a file is copied). */
const single = (r: TabRef) => r.kind !== 'file';

export interface Tab {
  key: string;
  ref: TabRef;
  preview?: boolean;
  /** Initial file mode (diff from the Git view). */
  mode?: 'diff';
  /** Line to reveal (from search); `nonce` changes on every reveal request. */
  line?: number;
  nonce?: number;
}

export interface Group {
  id: string;
  tabs: Tab[];
  active: string | null;
}

/** Split tree: a leaf is a group; a split lays children out in a row or a column. */
export type Node = { kind: 'leaf'; group: string } | { kind: 'split'; dir: 'row' | 'col'; children: Node[]; sizes: number[] };

export interface Layout {
  groups: Group[];
  root: Node;
  focused: string;
}

export type Side = 'left' | 'right' | 'up' | 'down';

export const MAX_GROUPS = 6;

export const tabKey = (r: TabRef) => (r.kind === 'file' ? `f:${r.repo}:${r.path}` : r.kind === 'session' ? `s:${r.id}` : r.kind === 'shell' ? `sh:${r.id}` : `t:${r.id}`);

let seq = 0;
const newId = () => `g${Date.now().toString(36)}${(seq++).toString(36)}`;

export const emptyLayout = (): Layout => {
  const id = newId();
  return { groups: [{ id, tabs: [], active: null }], root: { kind: 'leaf', group: id }, focused: id };
};

const mapGroup = (l: Layout, id: string, fn: (g: Group) => Group): Layout => ({ ...l, groups: l.groups.map((g) => (g.id === id ? fn(g) : g)) });

export const focusedGroup = (l: Layout) => l.groups.find((g) => g.id === l.focused) ?? l.groups[0]!;

export const activeTab = (g: Group) => g.tabs.find((t) => t.key === g.active) ?? null;

/** Where a tool tab is open (group id), if anywhere. */
export const findKey = (l: Layout, key: string) => l.groups.find((g) => g.tabs.some((t) => t.key === key))?.id ?? null;
export const findTool = (l: Layout, id: string) => findKey(l, `t:${id}`);

/** Groups in reading order (tree order) — for Alt+1…n and «next group». */
export function groupOrder(l: Layout): string[] {
  const out: string[] = [];
  const walk = (n: Node) => (n.kind === 'leaf' ? out.push(n.group) : n.children.forEach(walk));
  walk(l.root);
  return out;
}

// ---------- tree helpers ----------

function removeLeaf(n: Node, id: string): Node | null {
  if (n.kind === 'leaf') return n.group === id ? null : n;
  const kept: Node[] = [];
  const sizes: number[] = [];
  n.children.forEach((c, i) => {
    const r = removeLeaf(c, id);
    if (r) {
      kept.push(r);
      sizes.push(n.sizes[i] ?? 1);
    }
  });
  if (!kept.length) return null;
  if (kept.length === 1) return kept[0]!;
  const total = sizes.reduce((a, b) => a + b, 0);
  return normalize({ ...n, children: kept, sizes: sizes.map((s) => s / total) });
}

/** Merge a split child that has the same direction into its parent (row in a row → one row). */
function normalize(n: Node): Node {
  if (n.kind === 'leaf') return n;
  const children: Node[] = [];
  const sizes: number[] = [];
  n.children.forEach((c0, i) => {
    const c = normalize(c0);
    const s = n.sizes[i] ?? 1 / n.children.length;
    if (c.kind === 'split' && c.dir === n.dir) {
      c.children.forEach((cc, j) => {
        children.push(cc);
        sizes.push(s * (c.sizes[j] ?? 1 / c.children.length));
      });
    } else {
      children.push(c);
      sizes.push(s);
    }
  });
  return children.length === 1 ? children[0]! : { ...n, children, sizes };
}

/** Put group `id` next to `target` on a side. */
function insertNext(n: Node, target: string, id: string, side: Side): Node {
  const dir = side === 'left' || side === 'right' ? 'row' : 'col';
  const after = side === 'right' || side === 'down';
  const leaf: Node = { kind: 'leaf', group: id };
  if (n.kind === 'leaf') {
    if (n.group !== target) return n;
    return { kind: 'split', dir, children: after ? [n, leaf] : [leaf, n], sizes: [0.5, 0.5] };
  }
  const i = n.children.findIndex((c) => c.kind === 'leaf' && c.group === target);
  if (i >= 0 && n.dir === dir) {
    const children = [...n.children];
    const sizes = [...n.sizes];
    const half = (sizes[i] ?? 0.5) / 2;
    sizes[i] = half;
    children.splice(after ? i + 1 : i, 0, leaf);
    sizes.splice(after ? i + 1 : i, 0, half);
    return { ...n, children, sizes };
  }
  return { ...n, children: n.children.map((c) => insertNext(c, target, id, side)) };
}

/** Drop empty groups (keeping at least one) and collapse the tree. */
function prune(l: Layout): Layout {
  if (l.groups.length <= 1) return l;
  const empty = l.groups.filter((g) => !g.tabs.length).map((g) => g.id);
  if (!empty.length) return l;
  if (empty.length === l.groups.length) {
    const g = l.groups.find((x) => x.id === l.focused) ?? l.groups[0]!;
    return { groups: [g], root: { kind: 'leaf', group: g.id }, focused: g.id };
  }
  const order = groupOrder(l);
  let root: Node | null = l.root;
  for (const id of empty) root = root ? removeLeaf(root, id) : null;
  const groups = l.groups.filter((g) => g.tabs.length);
  let focused = l.focused;
  if (!groups.some((g) => g.id === focused)) {
    const i = order.indexOf(focused);
    focused = order.slice(0, Math.max(0, i)).reverse().find((id) => groups.some((g) => g.id === id)) ?? groups[0]!.id;
  }
  return { groups, root: root ?? { kind: 'leaf', group: groups[0]!.id }, focused };
}

// ---------- tabs ----------

export interface OpenOpts {
  preview?: boolean;
  /** Target group (default: the focused one). */
  group?: string;
  /** Insert position in the group (drag and drop). */
  index?: number;
  mode?: 'diff';
  line?: number;
  /** Key of a tab in the target group that the new one takes the place of (the Sessions list
   *  turns into the picked session). */
  replace?: string;
}

export function open(l: Layout, ref: TabRef, opts: OpenOpts = {}): Layout {
  const key = tabKey(ref);
  if (opts.replace && opts.replace !== key) {
    const gid = opts.group && l.groups.some((g) => g.id === opts.group) ? opts.group : focusedGroup(l).id;
    const g = l.groups.find((x) => x.id === gid)!;
    const at = g.tabs.findIndex((t) => t.key === opts.replace);
    if (at >= 0) {
      const { replace: _, ...rest } = opts;
      // already open in this group: just drop the replaced tab and show it
      if (g.tabs.some((t) => t.key === key)) return open(mapGroup(l, gid, (x) => without(x, new Set([opts.replace!]))), ref, rest);
      return mapGroup({ ...l, focused: gid }, gid, (x) => ({ ...x, tabs: x.tabs.map((t, i) => (i === at ? { key, ref } : t)), active: key }));
    }
  }
  const gid = opts.group && l.groups.some((g) => g.id === opts.group) ? opts.group : focusedGroup(l).id;
  const reveal = opts.line ? { line: opts.line, nonce: Date.now() } : {};
  return mapGroup({ ...l, focused: gid }, gid, (g) => {
    const existing = g.tabs.find((t) => t.key === key);
    if (existing) {
      const tabs = g.tabs.map((t) => (t.key === key ? { ...t, ...reveal, mode: opts.mode ?? t.mode, preview: t.preview && opts.preview !== false ? true : false } : t));
      return { ...g, tabs, active: key };
    }
    const tab: Tab = { key, ref, preview: Boolean(opts.preview), mode: opts.mode, ...reveal };
    const prev = opts.preview && opts.index === undefined ? g.tabs.findIndex((t) => t.preview) : -1;
    if (prev >= 0) {
      const tabs = [...g.tabs];
      tabs[prev] = tab;
      return { ...g, tabs, active: key };
    }
    // after the active tab (or at the end), unless dropped at a position
    const at = opts.index ?? (g.tabs.findIndex((t) => t.key === g.active) + 1 || g.tabs.length);
    const tabs = [...g.tabs];
    tabs.splice(Math.max(0, Math.min(tabs.length, at)), 0, tab);
    return { ...g, tabs, active: key };
  });
}

export const pin = (l: Layout, gid: string, key: string): Layout =>
  mapGroup(l, gid, (g) => (g.tabs.some((t) => t.key === key && t.preview) ? { ...g, tabs: g.tabs.map((t) => (t.key === key ? { ...t, preview: false } : t)) } : g));

export const activate = (l: Layout, gid: string, key: string): Layout => mapGroup({ ...l, focused: gid }, gid, (g) => ({ ...g, active: key }));

export const focus = (l: Layout, gid: string): Layout => (l.focused === gid || !l.groups.some((g) => g.id === gid) ? l : { ...l, focused: gid });

function without(g: Group, keys: Set<string>): Group {
  const tabs = g.tabs.filter((t) => !keys.has(t.key));
  let active = g.active;
  if (active && keys.has(active)) {
    const i = g.tabs.findIndex((t) => t.key === active);
    // the neighbour on the right, else on the left (like VS Code)
    active = g.tabs.slice(i + 1).find((t) => !keys.has(t.key))?.key ?? g.tabs.slice(0, i).reverse().find((t) => !keys.has(t.key))?.key ?? null;
  }
  return { ...g, tabs, active };
}

export const close = (l: Layout, gid: string, key: string): Layout => prune(mapGroup(l, gid, (g) => without(g, new Set([key]))));

export const closeOthers = (l: Layout, gid: string, key: string): Layout =>
  prune(mapGroup(l, gid, (g) => ({ ...without(g, new Set(g.tabs.filter((t) => t.key !== key).map((t) => t.key))), active: key })));

export const closeRight = (l: Layout, gid: string, key: string): Layout =>
  prune(
    mapGroup(l, gid, (g) => {
      const i = g.tabs.findIndex((t) => t.key === key);
      return without(g, new Set(g.tabs.slice(i + 1).map((t) => t.key)));
    }),
  );

export const closeAll = (l: Layout, gid: string): Layout => prune(mapGroup(l, gid, (g) => ({ ...g, tabs: [], active: null })));

/** Move a tab to a group at an index (reorder inside a group as well). */
export function move(l: Layout, from: string, key: string, to: string, index?: number): Layout {
  const src = l.groups.find((g) => g.id === from);
  const tab = src?.tabs.find((t) => t.key === key);
  if (!src || !tab || !l.groups.some((g) => g.id === to)) return l;
  let next = from === to ? l : mapGroup(l, from, (g) => without(g, new Set([key])));
  next = mapGroup({ ...next, focused: to }, to, (g) => {
    const rest = g.tabs.filter((t) => t.key !== key);
    const at = index === undefined ? rest.length : Math.max(0, Math.min(rest.length, from === to && g.tabs.findIndex((t) => t.key === key) < index ? index - 1 : index));
    rest.splice(at, 0, { ...tab, preview: false });
    return { ...g, tabs: rest, active: key };
  });
  return prune(next);
}

/** A new group with one tab, placed on a side of `target`. */
function addGroup(l: Layout, target: string, side: Side, tab: Tab): Layout {
  if (l.groups.length >= MAX_GROUPS || !l.groups.some((g) => g.id === target)) return l;
  const id = newId();
  return {
    groups: [...l.groups, { id, tabs: [{ ...tab, preview: false, nonce: undefined }], active: tab.key }],
    root: normalize(insertNext(l.root, target, id, side)),
    focused: id,
  };
}

/**
 * Split a group to a side. With a tab key: a tool moves to the new group, a file is
 * duplicated (as in VS Code). Without a key: the active tab of the group.
 */
export function split(l: Layout, gid: string, side: Side = 'right', key?: string): Layout {
  const src = l.groups.find((g) => g.id === gid);
  const tab = src?.tabs.find((t) => t.key === (key ?? src.active));
  if (!src || !tab) return l;
  let next = addGroup(l, gid, side, tab);
  if (next !== l && single(tab.ref)) next = mapGroup(next, gid, (g) => without(g, new Set([tab.key])));
  return prune(next);
}

/** Move a tab into a new group on a side of `to` (drag onto an edge). */
export function moveToNewGroup(l: Layout, from: string, key: string, to: string, side: Side): Layout {
  const tab = l.groups.find((g) => g.id === from)?.tabs.find((t) => t.key === key);
  if (!tab) return l;
  // the only tab of the target group dropped on its own edge — nothing to split
  if (from === to && l.groups.find((g) => g.id === from)!.tabs.length === 1) return l;
  const next = addGroup(l, to, side, tab);
  return next === l ? l : prune(mapGroup(next, from, (g) => without(g, new Set([key]))));
}

/** Open a reference (a file dragged from the tree) in a new group on a side of `to`. */
export function openInNewGroup(l: Layout, ref: TabRef, to: string, side: Side): Layout {
  return addGroup(l, to, side, { key: tabKey(ref), ref });
}

// ---------- files renamed / deleted in the tree ----------

const under = (p: string, base: string) => p === base || p.startsWith(`${base}/`);

/** A file or folder was renamed/moved: retarget its open tabs. */
export function renamePath(l: Layout, repo: string, from: string, to: string): Layout {
  return {
    ...l,
    groups: l.groups.map((g) => {
      let active = g.active;
      const seen = new Set<string>();
      const tabs: Tab[] = [];
      for (const t of g.tabs) {
        let tab = t;
        if (t.ref.kind === 'file' && t.ref.repo === repo && under(t.ref.path, from)) {
          const ref: TabRef = { kind: 'file', repo, path: to + t.ref.path.slice(from.length) };
          tab = { ...t, ref, key: tabKey(ref) };
          if (g.active === t.key) active = tab.key;
        }
        if (seen.has(tab.key)) continue;
        seen.add(tab.key);
        tabs.push(tab);
      }
      return { ...g, tabs, active };
    }),
  };
}

/** A file or folder was deleted: close its tabs. */
export function closePath(l: Layout, repo: string, path: string): Layout {
  return prune({
    ...l,
    groups: l.groups.map((g) =>
      without(g, new Set(g.tabs.filter((t) => t.ref.kind === 'file' && t.ref.repo === repo && under(t.ref.path, path)).map((t) => t.key))),
    ),
  });
}

// ---------- geometry ----------

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Divider between children i and i+1 of the split at `path` (fractions of the whole area). */
export interface Divider {
  path: number[];
  index: number;
  dir: 'row' | 'col';
  /** Line position and extent. */
  rect: Rect;
  /** The split's own rect (to convert pixels to its share). */
  parent: Rect;
}

/** Rectangles of groups (fractions 0…1) and the dividers between them. */
export function geometry(root: Node): { rects: Map<string, Rect>; dividers: Divider[] } {
  const rects = new Map<string, Rect>();
  const dividers: Divider[] = [];
  const walk = (n: Node, r: Rect, path: number[]) => {
    if (n.kind === 'leaf') return void rects.set(n.group, r);
    const total = n.sizes.reduce((a, b) => a + b, 0) || 1;
    let at = 0;
    n.children.forEach((c, i) => {
      const share = (n.sizes[i] ?? 0) / total;
      const cr = n.dir === 'row' ? { x: r.x + at * r.w, y: r.y, w: share * r.w, h: r.h } : { x: r.x, y: r.y + at * r.h, w: r.w, h: share * r.h };
      walk(c, cr, [...path, i]);
      at += share;
      if (i < n.children.length - 1)
        dividers.push({
          path,
          index: i,
          dir: n.dir,
          parent: r,
          rect: n.dir === 'row' ? { x: r.x + at * r.w, y: r.y, w: 0, h: r.h } : { x: r.x, y: r.y + at * r.h, w: r.w, h: 0 },
        });
    });
  };
  walk(root, { x: 0, y: 0, w: 1, h: 1 }, []);
  return { rects, dividers };
}

/** Drag a divider by `delta` — a share of that split's size. */
export function resize(l: Layout, path: number[], index: number, delta: number): Layout {
  const upd = (n: Node, depth: number): Node => {
    if (n.kind === 'leaf') return n;
    if (depth < path.length) return { ...n, children: n.children.map((c, i) => (i === path[depth] ? upd(c, depth + 1) : c)) };
    const sizes = [...n.sizes];
    const total = sizes.reduce((a, b) => a + b, 0) || 1;
    const a = sizes[index]!;
    const b = sizes[index + 1]!;
    const min = 0.12 * total;
    const na = Math.max(min, Math.min(a + b - min, a + delta * total));
    sizes[index] = na;
    sizes[index + 1] = a + b - na;
    return { ...n, sizes };
  };
  return { ...l, root: upd(l.root, 0) };
}

// ---------- persistence ----------

function validNode(n: unknown, ids: Set<string>, seen: Set<string>): Node | null {
  const x = n as Node;
  if (x?.kind === 'leaf') {
    if (!ids.has(x.group) || seen.has(x.group)) return null;
    seen.add(x.group);
    return { kind: 'leaf', group: x.group };
  }
  if (x?.kind === 'split' && (x.dir === 'row' || x.dir === 'col') && Array.isArray(x.children)) {
    const children: Node[] = [];
    const sizes: number[] = [];
    x.children.forEach((c, i) => {
      const v = validNode(c, ids, seen);
      if (v) {
        children.push(v);
        sizes.push(Number(x.sizes?.[i]) > 0 ? Number(x.sizes[i]) : 1);
      }
    });
    if (!children.length) return null;
    return children.length === 1 ? children[0]! : { kind: 'split', dir: x.dir, children, sizes };
  }
  return null;
}

/** Drop tabs whose tool no longer exists, fix broken state, upgrade the old row-only format. */
export function sanitize(l: Partial<Layout> | null | undefined, tools: Set<string>, opts: { restore?: boolean } = {}): Layout {
  if (!l || !Array.isArray(l.groups) || !l.groups.length) return emptyLayout();
  const groups: Group[] = l.groups
    .filter((g) => g && typeof g.id === 'string' && Array.isArray(g.tabs))
    .map((g) => {
      const seen = new Set<string>();
      const tabs = g.tabs.filter((t) => {
        if (!t?.ref) return false;
        if (t.ref.kind === 'tool' && !tools.has(t.ref.id)) return false;
        // unsent drafts of new sessions are not restored after a reload
        if (opts.restore && t.ref.kind === 'session' && t.ref.id.startsWith('new:')) return false;
        // a key at most once per group
        if (seen.has(t.key)) return false;
        seen.add(t.key);
        return true;
      });
      return { id: g.id, tabs: tabs.map((t) => ({ ...t, nonce: undefined, line: undefined })), active: tabs.some((t) => t.key === g.active) ? g.active : (tabs[0]?.key ?? null) };
    })
    .slice(0, MAX_GROUPS);
  if (!groups.length) return emptyLayout();
  const ids = new Set(groups.map((g) => g.id));
  let root = validNode(l.root, ids, new Set());
  // old format (groups side by side, no tree) or groups missing from the tree → one row
  const inTree = root ? new Set(groupOrder({ groups, root, focused: '' })) : new Set<string>();
  const missing = groups.filter((g) => !inTree.has(g.id));
  if (!root || missing.length) {
    const leaves: Node[] = [...(root ? [root] : []), ...missing.map((g) => ({ kind: 'leaf' as const, group: g.id }))];
    root = leaves.length === 1 ? leaves[0]! : normalize({ kind: 'split', dir: 'row', children: leaves, sizes: leaves.map(() => 1 / leaves.length) });
  }
  return prune({ groups, root, focused: ids.has(l.focused ?? '') ? l.focused! : groups[0]!.id });
}

/** A draft session tab got its real id: retarget it in place (same group and position). */
export function retarget(l: Layout, from: TabRef, to: TabRef): Layout {
  const fk = tabKey(from);
  const tk = tabKey(to);
  return {
    ...l,
    groups: l.groups.map((g) =>
      g.tabs.some((t) => t.key === fk) ? { ...g, tabs: g.tabs.map((t) => (t.key === fk ? { ...t, key: tk, ref: to } : t)), active: g.active === fk ? tk : g.active } : g,
    ),
  };
}

/** Close a tab in every group (a shell that exited). */
export function closeKey(l: Layout, key: string): Layout {
  if (!findKey(l, key)) return l;
  return prune({ ...l, groups: l.groups.map((g) => (g.tabs.some((t) => t.key === key) ? without(g, new Set([key])) : g)) });
}

/** Ids of the shell jobs that have a tab somewhere. */
export const shellIds = (l: Layout) => new Set(l.groups.flatMap((g) => g.tabs.flatMap((t) => (t.ref.kind === 'shell' ? [t.ref.id] : []))));
