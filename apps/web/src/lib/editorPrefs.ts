import { useSyncExternalStore } from 'react';

/** Editor preferences (per browser): Vim keybindings, code font size, indentation, relative line numbers. */

const KEY = 'hub.editor.vim';
const listeners = new Set<() => void>();

function read(): boolean {
  try {
    return localStorage.getItem(KEY) === '1';
  } catch {
    return false;
  }
}

let vim = read();

export function setVimMode(on: boolean) {
  vim = on;
  try {
    localStorage.setItem(KEY, on ? '1' : '0');
  } catch {}
  for (const l of listeners) l();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

export const useVimMode = () => useSyncExternalStore(subscribe, () => vim);

// ---------- code font size (editor, file view, diffs) ----------

const SIZE_KEY = 'hub.editor.fontSize';
export const CODE_SIZE = { min: 10, max: 24, default: 12 };
function readSize(): number {
  try {
    const n = Number(localStorage.getItem(SIZE_KEY));
    return n >= CODE_SIZE.min && n <= CODE_SIZE.max ? n : CODE_SIZE.default;
  } catch {
    return CODE_SIZE.default;
  }
}
let codeSize = readSize();
/** Code views read the size from the --code-size CSS variable: one place to change it. */
const applySize = () => document.documentElement.style.setProperty('--code-size', `${codeSize}px`);
applySize();

export function setCodeSize(px: number) {
  codeSize = Math.min(CODE_SIZE.max, Math.max(CODE_SIZE.min, Math.round(px)));
  try {
    localStorage.setItem(SIZE_KEY, String(codeSize));
  } catch {}
  applySize();
  for (const l of listeners) l();
}

export const useCodeSize = () => useSyncExternalStore(subscribe, () => codeSize);

// ---------- indentation and relative line numbers ----------

const TAB_KEY = 'hub.editor.tabSize';
export const TAB_SIZES = [2, 4, 8] as const;
export type TabSize = (typeof TAB_SIZES)[number];
function readTab(): TabSize {
  try {
    const n = Number(localStorage.getItem(TAB_KEY));
    return (TAB_SIZES as readonly number[]).includes(n) ? (n as TabSize) : 2;
  } catch {
    return 2;
  }
}
let tabSize = readTab();
/** Read-only views (file view, diffs) show tab characters with the same width. */
const applyTab = () => document.documentElement.style.setProperty('--code-tab', String(tabSize));
applyTab();

export function setTabSize(n: TabSize) {
  tabSize = n;
  try {
    localStorage.setItem(TAB_KEY, String(n));
  } catch {}
  applyTab();
  for (const l of listeners) l();
}
export const useTabSize = () => useSyncExternalStore(subscribe, () => tabSize);

const REL_KEY = 'hub.editor.relativeLines';
function readRel(): boolean {
  try {
    return localStorage.getItem(REL_KEY) === '1';
  } catch {
    return false;
  }
}
let relativeLines = readRel();
export function setRelativeLines(on: boolean) {
  relativeLines = on;
  try {
    localStorage.setItem(REL_KEY, on ? '1' : '0');
  } catch {}
  for (const l of listeners) l();
}
export const useRelativeLines = () => useSyncExternalStore(subscribe, () => relativeLines);

// ---------- side bar position (desktop workspace) ----------

const SIDE_KEY = 'hub.workspace.sideRight';
function readSide(): boolean {
  try {
    return localStorage.getItem(SIDE_KEY) === '1';
  } catch {
    return false;
  }
}
let sideRight = readSide();
/** Explorer / Search / Git on the right of the editor instead of the left. */
export function setSideRight(on: boolean) {
  sideRight = on;
  try {
    localStorage.setItem(SIDE_KEY, on ? '1' : '0');
  } catch {}
  for (const l of listeners) l();
}
export const useSideRight = () => useSyncExternalStore(subscribe, () => sideRight);
