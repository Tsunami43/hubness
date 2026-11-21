import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { Compartment, EditorState, type Extension } from '@codemirror/state';
import {
  crosshairCursor,
  drawSelection,
  dropCursor,
  EditorView,
  gutter,
  GutterMarker,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  rectangularSelection,
} from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { highlightSelectionMatches, searchKeymap } from '@codemirror/search';
import { bracketMatching, foldGutter, foldKeymap, HighlightStyle, indentOnInput, indentUnit, LanguageDescription, syntaxHighlighting } from '@codemirror/language';
import { languages } from '@codemirror/language-data';
import { tags } from '@lezer/highlight';
import { getCM, Vim, vim } from '@replit/codemirror-vim';
import { t } from '../i18n';
import { AskActions, codeSnippet } from './SelectionAsk';

/**
 * Text editor for project files (CodeMirror 6, loaded lazily). Colors come from the app
 * theme (CSS variables, the same --syntax-* roles as the viewer), language support is loaded
 * by file name. Vim keybindings are optional; `:w` and Ctrl/Cmd+S save.
 */

const v = (role: string) => `var(--syntax-${role})`;

const highlight = HighlightStyle.define([
  { tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docComment], color: v('comment') },
  { tag: [tags.keyword, tags.controlKeyword, tags.moduleKeyword, tags.operatorKeyword, tags.definitionKeyword, tags.modifier, tags.self], color: v('keyword') },
  { tag: [tags.string, tags.special(tags.string), tags.regexp, tags.character, tags.docString], color: v('string') },
  { tag: [tags.number, tags.bool, tags.null, tags.atom, tags.constant(tags.name), tags.standard(tags.name), tags.escape], color: v('constant') },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.macroName], color: v('function') },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.definition(tags.typeName), tags.annotation], color: v('entity') },
  { tag: [tags.tagName, tags.angleBracket], color: v('tag') },
  { tag: [tags.attributeName, tags.propertyName], color: v('constant') },
  { tag: tags.heading, color: v('constant'), fontWeight: 'bold' },
  { tag: tags.emphasis, fontStyle: 'italic' },
  { tag: tags.strong, fontWeight: 'bold' },
  { tag: tags.link, color: v('string'), textDecoration: 'underline' },
  { tag: tags.invalid, color: 'var(--color-danger)' },
  { tag: [tags.inserted], color: 'var(--color-add-fg)' },
  { tag: [tags.deleted], color: 'var(--color-del-fg)' },
]);

const mix = (c: string, pct: number) => `color-mix(in srgb, var(--color-${c}) ${pct}%, transparent)`;

const theme = EditorView.theme({
  '&': { height: '100%', fontSize: 'var(--code-size, 12px)', backgroundColor: 'var(--color-bg)', color: 'var(--color-fg)' },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { fontFamily: 'var(--font-mono)', lineHeight: '1.6' },
  '.cm-content': { caretColor: 'var(--color-accent)', paddingBottom: '40vh' },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--color-accent)', borderLeftWidth: '2px' },
  '.cm-gutters': { backgroundColor: 'var(--color-bg)', color: mix('muted', 60), border: 'none' },
  '.cm-activeLine': { backgroundColor: mix('sunken', 70) },
  '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--color-fg)' },
  '.cm-selectionBackground, &.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-content ::selection': {
    backgroundColor: `${mix('accent', 28)} !important`,
  },
  '.cm-selectionMatch': { backgroundColor: mix('accent', 14) },
  '.cm-matchingBracket, &.cm-focused .cm-matchingBracket': { backgroundColor: mix('accent', 22), outline: `1px solid ${mix('accent', 50)}` },
  '.cm-searchMatch': { backgroundColor: mix('warn', 30) },
  '.cm-searchMatch-selected': { backgroundColor: mix('warn', 55) },
  '.cm-foldPlaceholder': { backgroundColor: 'var(--color-sunken)', border: '1px solid var(--color-line)', color: 'var(--color-muted)' },
  '.cm-panels': { backgroundColor: 'var(--color-surface)', color: 'var(--color-fg)' },
  '.cm-panels-bottom': { borderTop: '1px solid var(--color-line)' },
  '.cm-panels-top': { borderBottom: '1px solid var(--color-line)' },
  '.cm-panel input, .cm-panel button': { font: 'inherit', fontSize: 'var(--code-size, 12px)' },
  '.cm-textfield': { backgroundColor: 'var(--color-bg)', border: '1px solid var(--color-line)', borderRadius: '0', color: 'var(--color-fg)' },
  '.cm-button': { backgroundImage: 'none', backgroundColor: 'var(--color-sunken)', border: '1px solid var(--color-line)', borderRadius: '0', color: 'var(--color-fg)' },
  // Vim: block cursor and the command line
  '.cm-fat-cursor': { background: 'var(--color-accent) !important', color: 'var(--color-bg) !important' },
  '&:not(.cm-focused) .cm-fat-cursor': { background: 'none !important', outline: '1px solid var(--color-accent)', color: 'transparent !important' },
  '.cm-vim-panel': { padding: '2px 8px', fontFamily: 'var(--font-mono)' },
  '.cm-vim-panel input': { color: 'var(--color-fg)', fontFamily: 'var(--font-mono)' },
});

/** A line number in the gutter. */
class NumberMarker extends GutterMarker {
  constructor(readonly text: string) {
    super();
  }
  eq(other: NumberMarker) {
    return other.text === this.text;
  }
  toDOM() {
    return document.createTextNode(this.text);
  }
}

/**
 * Relative numbers (Vim `relativenumber` + `number`): the cursor line shows its own number,
 * the others their distance to it — for 5j / 3dk. Redrawn when the cursor moves.
 */
const relativeNumbers = gutter({
  class: 'cm-lineNumbers',
  lineMarker(view, line) {
    const cur = view.state.doc.lineAt(view.state.selection.main.head).number;
    const n = view.state.doc.lineAt(line.from).number;
    return new NumberMarker(String(n === cur ? n : Math.abs(n - cur)));
  },
  lineMarkerChange: (u) => u.selectionSet || u.docChanged,
  initialSpacer: (view) => new NumberMarker(String(view.state.doc.lines)),
});

const numbersExt = (relative: boolean): Extension => (relative ? relativeNumbers : lineNumbers());
const indentExt = (size: number): Extension => [indentUnit.of(' '.repeat(size)), EditorState.tabSize.of(size)];

const base: Extension = [
  highlightActiveLineGutter(),
  highlightSpecialChars(),
  history(),
  foldGutter(),
  drawSelection(),
  dropCursor(),
  EditorState.allowMultipleSelections.of(true),
  indentOnInput(),
  syntaxHighlighting(highlight),
  bracketMatching(),
  rectangularSelection(),
  crosshairCursor(),
  highlightActiveLine(),
  highlightSelectionMatches(),
  keymap.of([...defaultKeymap, ...searchKeymap, ...historyKeymap, ...foldKeymap, indentWithTab]),
  theme,
];

// `:w` in Vim mode calls the save of the editor it was typed in
const saves = new WeakMap<EditorView, () => void>();

/** Vim mode as the status line shows it. */
const VIM_MODES: Record<string, { label: string; cls: string }> = {
  normal: { label: 'NORMAL', cls: 'text-accent' },
  insert: { label: 'INSERT', cls: 'text-ok' },
  visual: { label: 'VISUAL', cls: 'text-warn' },
  'visual line': { label: 'V-LINE', cls: 'text-warn' },
  'visual block': { label: 'V-BLOCK', cls: 'text-warn' },
  replace: { label: 'REPLACE', cls: 'text-danger' },
};

interface Cursor {
  line: number;
  col: number;
  lines: number;
  /** Selected characters (main selection). */
  selected: number;
  from: number;
  to: number;
  /** The end the selection was extended to (where the mouse let go, where Vim moved). */
  head: number;
}

const cursorOf = (s: EditorState): Cursor => {
  const { head, from, to } = s.selection.main;
  const line = s.doc.lineAt(head);
  return { line: line.number, col: head - line.from + 1, lines: s.doc.lines, selected: to - from, from, to, head };
};
Vim.defineEx('write', 'w', (cm: { cm6: EditorView }) => saves.get(cm.cm6)?.());

export default function CodeEditor({
  path,
  initial,
  line,
  vimMode,
  autoFocus = true,
  tabSize = 2,
  relativeLines = false,
  external,
  onChange,
  onSave,
}: {
  path: string;
  /** Initial text; the editor owns the document afterwards (remount with a new key to replace it). */
  initial: string;
  /** Put the cursor on this line when opening. */
  line?: number;
  vimMode: boolean;
  /** Focus on open (off when a file opens straight in the editor from the tree). */
  autoFocus?: boolean;
  /** Spaces per indent level (Tab) and the width of a tab character. */
  tabSize?: number;
  /** Line numbers relative to the cursor line. */
  relativeLines?: boolean;
  /**
   * Replace the document from outside (the file changed on disk and there are no own edits):
   * applied when `rev` changes, as a minimal edit so the cursor and scroll stay in place.
   */
  external?: { text: string; rev: number };
  onChange: (text: string) => void;
  onSave: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const vimSlot = useRef(new Compartment());
  const langSlot = useRef(new Compartment());
  const numbersSlot = useRef(new Compartment());
  const indentSlot = useRef(new Compartment());
  const [cursor, setCursor] = useState<Cursor>({ line: 1, col: 1, lines: 1, selected: 0, from: 0, to: 0, head: 0 });
  // the ask bar: at the selection's end, once the mouse is let go; follows scrolling
  const askBar = useRef<HTMLDivElement>(null);
  const wrap = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  const [, setScrollTick] = useState(0);
  const [vimState, setVimState] = useState('normal');
  const cb = useRef({ onChange, onSave });
  cb.current = { onChange, onSave };

  useEffect(() => {
    const save = () => {
      cb.current.onSave();
      return true;
    };
    const ed = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: initial,
        extensions: [
          // Vim goes first: its keymap must take precedence
          vimSlot.current.of(vimMode ? vim() : []),
          keymap.of([{ key: 'Mod-s', run: save, preventDefault: true }]),
          numbersSlot.current.of(numbersExt(relativeLines)),
          indentSlot.current.of(indentExt(tabSize)),
          base,
          langSlot.current.of([]),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) cb.current.onChange(u.state.doc.toString());
            if (u.docChanged || u.selectionSet) setCursor(cursorOf(u.state));
          }),
        ],
      }),
    });
    view.current = ed;
    setCursor(cursorOf(ed.state));
    saves.set(ed, () => cb.current.onSave());
    if (line && line <= ed.state.doc.lines) {
      const at = ed.state.doc.line(line).from;
      ed.dispatch({ selection: { anchor: at }, effects: EditorView.scrollIntoView(at, { y: 'center' }) });
    }
    if (autoFocus) ed.focus();
    let alive = true;
    const lang = LanguageDescription.matchFilename(languages, path.split('/').pop() ?? path);
    lang?.load().then((support) => {
      if (alive) ed.dispatch({ effects: langSlot.current.reconfigure(support) });
    }, () => {});
    return () => {
      alive = false;
      ed.destroy();
      view.current = null;
    };
    // the document is owned by the editor; a new file remounts the component
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const ed = view.current;
    if (!ed) return;
    ed.dispatch({ effects: vimSlot.current.reconfigure(vimMode ? vim() : []) });
    const cm = vimMode ? getCM(ed) : null;
    if (!cm) return;
    setVimState(cm.state.vim?.insertMode ? 'insert' : 'normal');
    const onMode = (e: { mode: string; subMode?: string }) => setVimState(e.subMode ? `${e.mode} ${e.subMode === 'linewise' ? 'line' : 'block'}` : e.mode);
    cm.on('vim-mode-change', onMode);
    return () => cm.off('vim-mode-change', onMode);
  }, [vimMode]);
  useEffect(() => {
    view.current?.dispatch({ effects: numbersSlot.current.reconfigure(numbersExt(relativeLines)) });
  }, [relativeLines]);
  useEffect(() => {
    view.current?.dispatch({ effects: indentSlot.current.reconfigure(indentExt(tabSize)) });
  }, [tabSize]);

  useEffect(() => {
    const ed = view.current;
    if (!ed || !external) return;
    const cur = ed.state.doc.toString();
    const next = external.text;
    if (cur === next) return;
    // only the differing middle is replaced: selections before/after it map cleanly
    let a = 0;
    while (a < cur.length && a < next.length && cur[a] === next[a]) a++;
    let b = 0;
    while (b < cur.length - a && b < next.length - a && cur[cur.length - 1 - b] === next[next.length - 1 - b]) b++;
    ed.dispatch({ changes: { from: a, to: cur.length - b, insert: next.slice(a, next.length - b) } });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [external?.rev]);

  useEffect(() => {
    const sc = view.current?.scrollDOM;
    if (!sc) return;
    const on = () => setScrollTick((n) => n + 1);
    sc.addEventListener('scroll', on, { passive: true });
    return () => sc.removeEventListener('scroll', on);
  }, []);
  useEffect(() => {
    if (!dragging) return;
    const up = () => setDragging(false);
    window.addEventListener('pointerup', up);
    return () => window.removeEventListener('pointerup', up);
  }, [dragging]);

  const mode = VIM_MODES[vimState] ?? { label: vimState.toUpperCase(), cls: 'text-muted' };
  // the selection as a question to the agent (path, lines, code)
  const doc = view.current?.state.doc;
  const snippet =
    cursor.selected && doc && cursor.to <= doc.length
      ? codeSnippet(path, doc.lineAt(cursor.from).number, doc.lineAt(cursor.to).number, doc.sliceString(cursor.from, cursor.to))
      : '';
  // where the selection ends, relative to the wrapper (null: scrolled out of view)
  let edge: { x: number; top: number; bottom: number; above: boolean } | null = null;
  const ed = view.current;
  if (snippet && !dragging && ed && wrap.current) {
    const c = ed.coordsAtPos(cursor.head);
    const r = wrap.current.getBoundingClientRect();
    const sc = ed.scrollDOM.getBoundingClientRect();
    if (c && c.bottom > sc.top && c.top < sc.bottom) edge = { x: c.left - r.left, top: c.top - r.top, bottom: c.bottom - r.top, above: cursor.head === cursor.from };
  }
  // keep it inside the editor: under the end line (over it for an upward selection or near the bottom)
  useLayoutEffect(() => {
    const el = askBar.current;
    const w = wrap.current;
    if (!el || !w || !edge) return;
    const W = w.clientWidth;
    const H = w.clientHeight - 24;
    const { offsetWidth: bw, offsetHeight: bh } = el;
    el.style.left = `${Math.max(4, Math.min(edge.x - 12, W - bw - 4))}px`;
    const below = edge.bottom + 4;
    const over = edge.top - bh - 4;
    el.style.top = `${(edge.above && over >= 4) || below + bh > H - 4 ? Math.max(4, over) : below}px`;
  });
  return (
    <div ref={wrap} className="relative flex h-full min-h-0 flex-col">
      <div
        ref={host}
        className="min-h-0 flex-1 overflow-hidden [&_.cm-editor]:h-full"
        onPointerDown={(e) => e.button === 0 && (e.target as Element).closest('.cm-content') && setDragging(true)}
      />
      {edge ? (
        <div ref={askBar} className="absolute z-20 border border-line bg-surface text-xs shadow-xl shadow-black/40" style={{ left: edge.x, top: edge.bottom }}>
          <AskActions text={snippet} up={edge.above} />
        </div>
      ) : null}
      {/* status line: Vim mode on the left, where the cursor is on the right */}
      {/* one line whatever the width (a narrow split): the less important parts drop out first */}
      <div className="@container flex h-6 min-w-0 shrink-0 items-center gap-3 border-t border-line bg-surface px-3 font-mono text-[11px] whitespace-nowrap text-muted select-none">
        {vimMode ? <span className={`shrink-0 font-semibold ${mode.cls}`}>-- {mode.label} --</span> : null}
        <span className="ml-auto" />
        {cursor.selected ? <span className="hidden shrink-0 @xl:inline">{t('editor.selected', { n: cursor.selected })}</span> : null}
        <span className="shrink-0" title={t('editor.position')}>
          {t('editor.lnCol', { line: cursor.line, col: cursor.col })}
        </span>
        <span className="hidden shrink-0 @md:inline">{t('editor.lines', { n: cursor.lines, pct: Math.round((cursor.line / cursor.lines) * 100) })}</span>
      </div>
    </div>
  );
}
