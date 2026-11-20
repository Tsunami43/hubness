import { useEffect, useRef, useSyncExternalStore } from 'react';

/**
 * Leader-key shortcuts (like which-key in Neovim / Vimium hints).
 *
 * Tap the leader key on its own (Ctrl by default; Alt or Super can be chosen) — leader mode
 * turns on: a hint panel lists the keys, tabs and the activity bar show jump labels; the next
 * key runs the action, Esc or a click cancels. A tap is "pressed and released with nothing
 * else in between", so Ctrl+C and other combos are unaffected.
 *
 * Screens register their actions with useKeyActions(); the panel shows whatever is
 * registered right now. Preferences live in this browser (keyboards differ per device).
 */

export type LeaderKey = 'Control' | 'Alt' | 'Meta';

export interface KeyPrefs {
  enabled: boolean;
  leader: LeaderKey;
  /** Hint panel and jump labels while in leader mode. */
  hints: boolean;
}

export interface KeyAction {
  /** The key as KeyboardEvent.key: 'e', 'E' (Shift), '1', ','. */
  key: string;
  label: string;
  section: string;
  run: () => void;
}

const PREFS_KEY = 'hub.keys';
const DEFAULTS: KeyPrefs = { enabled: true, leader: 'Control', hints: true };

function readPrefs(): KeyPrefs {
  try {
    return { ...DEFAULTS, ...(JSON.parse(localStorage.getItem(PREFS_KEY) ?? '{}') as Partial<KeyPrefs>) };
  } catch {
    return DEFAULTS;
  }
}

// ---------- tiny stores ----------

let prefs = readPrefs();
let snapshot: KeyAction[] = [];
let active = false;
const registry = new Map<string, KeyAction[]>();
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

export function setKeyPrefs(patch: Partial<KeyPrefs>) {
  prefs = { ...prefs, ...patch };
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(prefs));
  } catch {}
  if (!prefs.enabled) active = false;
  emit();
}

export const useKeyPrefs = () => useSyncExternalStore(subscribe, () => prefs);
/** Leader mode is on (show jump labels). */
export const useLeader = () => useSyncExternalStore(subscribe, () => active);
export const useKeyActionList = () => useSyncExternalStore(subscribe, () => snapshot);

export function setLeader(on: boolean) {
  if (active === on) return;
  active = on;
  jumps = on ? collectJumps() : [];
  emit();
}

// ---------- jump hints (Vimium-style) ----------

/**
 * Any visible element marked `data-jump` (a project, a menu item, a session…) gets a letter
 * while in leader mode; pressing it clicks the element. Letters taken by the screen's own
 * actions are skipped.
 */
export interface Jump {
  key: string;
  el: HTMLElement;
  rect: DOMRect;
}

const POOL = 'asdfghjklqwertyuiopzxcvbnm';
let jumps: Jump[] = [];
export const useJumps = () => useSyncExternalStore(subscribe, () => jumps);

function collectJumps(): Jump[] {
  const taken = new Set(snapshot.map((a) => a.key));
  const free = [...POOL].filter((k) => !taken.has(k));
  const out: Jump[] = [];
  for (const el of document.querySelectorAll<HTMLElement>('[data-jump]')) {
    if (out.length >= free.length) break;
    const r = el.getBoundingClientRect();
    const visible = r.width > 0 && r.height > 0 && r.bottom > 0 && r.right > 0 && r.top < window.innerHeight && r.left < window.innerWidth && el.offsetParent !== null;
    if (visible) out.push({ key: free[out.length]!, el, rect: r });
  }
  return out;
}

function rebuild() {
  // later registrations (inner screens) win over earlier ones (global navigation)
  const byKey = new Map<string, KeyAction>();
  for (const list of registry.values()) for (const a of list) byKey.set(a.key, a);
  snapshot = [...byKey.values()];
  emit();
}

/** Register actions while the component is mounted. `actions` may change every render. */
export function useKeyActions(id: string, actions: KeyAction[]) {
  const ref = useRef(actions);
  ref.current = actions;
  const sig = actions.map((a) => `${a.key}:${a.label}:${a.section}`).join('|');
  useEffect(() => {
    // run through the ref: closures stay fresh without re-registering on every render
    registry.set(
      id,
      ref.current.map((a) => ({ ...a, run: () => ref.current.find((x) => x.key === a.key)?.run() })),
    );
    rebuild();
    return () => {
      registry.delete(id);
      rebuild();
    };
  }, [id, sig]);
}

// ---------- keyboard ----------

const MODIFIERS = new Set(['Control', 'Alt', 'Meta', 'Shift', 'AltGraph', 'CapsLock', 'OS']);
const TAP_MS = 600;

/** Install once (Layout). Capture phase: works over the terminal and the editor too. */
export function installLeaderKeys(): () => void {
  let tap: { at: number } | null = null;
  const onDown = (e: KeyboardEvent) => {
    if (!prefs.enabled) return;
    if (active) {
      if (MODIFIERS.has(e.key)) return;
      e.preventDefault();
      e.stopPropagation();
      const jump = jumps.find((j) => j.key === e.key);
      setLeader(false);
      if (e.key === 'Escape') return;
      const action = snapshot.find((a) => a.key === e.key);
      if (action) action.run();
      else if (jump) jump.el.click();
      return;
    }
    // the leader pressed alone starts a tap; anything else cancels it
    const alone = e.key === prefs.leader && !e.repeat && !(e.shiftKey || (e.ctrlKey && prefs.leader !== 'Control') || (e.altKey && prefs.leader !== 'Alt') || (e.metaKey && prefs.leader !== 'Meta'));
    tap = alone ? { at: Date.now() } : null;
  };
  const onUp = (e: KeyboardEvent) => {
    if (!prefs.enabled || e.key !== prefs.leader || !tap) return;
    const quick = Date.now() - tap.at < TAP_MS;
    tap = null;
    if (quick) setLeader(!active);
  };
  const cancel = () => {
    tap = null;
    setLeader(false);
  };
  window.addEventListener('keydown', onDown, true);
  window.addEventListener('keyup', onUp, true);
  window.addEventListener('mousedown', cancel, true);
  window.addEventListener('blur', cancel);
  return () => {
    window.removeEventListener('keydown', onDown, true);
    window.removeEventListener('keyup', onUp, true);
    window.removeEventListener('mousedown', cancel, true);
    window.removeEventListener('blur', cancel);
  };
}

/** How a key is shown in hints. */
export const keyLabel = (k: string) => (k === ' ' ? 'Space' : k.length === 1 && k !== k.toLowerCase() ? `⇧${k.toLowerCase()}` : k);

export const leaderLabel = (l: LeaderKey) => (l === 'Control' ? 'Ctrl' : l === 'Meta' ? 'Super' : 'Alt');
