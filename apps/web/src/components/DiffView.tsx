import { useMemo } from 'react';
import { t } from '../i18n';
import { useEffect, useState } from 'react';
import { CodeLine } from './Code';
import { tokenize, type Token } from '../lib/highlight';

/** Renders a unified diff: line numbers, colors, horizontal scroll without wrapping. */

interface Line {
  type: 'add' | 'del' | 'ctx' | 'hunk' | 'meta';
  text: string;
  old?: number;
  new?: number;
  /** File the line belongs to (from the diff --git / +++ headers). */
  file?: string;
}

function parse(diff: string): Line[] {
  const out: Line[] = [];
  let o = 0;
  let n = 0;
  let inHunk = false;
  let file: string | undefined;
  for (const raw of diff.split('\n')) {
    const git = raw.match(/^diff --git a\/(.+?) b\/(.+)$/);
    if (git) file = git[2];
    if (git || raw.startsWith('index ') || raw.startsWith('new file') || raw.startsWith('deleted file')) {
      inHunk = false;
      continue;
    }
    if (!inHunk && raw.startsWith('+++ ') && raw.slice(4) !== '/dev/null') file = raw.slice(4).replace(/^b\//, '');
    if (!inHunk && (raw.startsWith('--- ') || raw.startsWith('+++ '))) continue;
    const m = raw.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/);
    if (m) {
      o = Number(m[1]);
      n = Number(m[2]);
      inHunk = true;
      out.push({ type: 'hunk', text: raw });
      continue;
    }
    if (!inHunk) {
      if (raw.trim()) out.push({ type: 'meta', text: raw });
      continue;
    }
    if (raw.startsWith('+')) out.push({ type: 'add', text: raw.slice(1), new: n++ , file });
    else if (raw.startsWith('-')) out.push({ type: 'del', text: raw.slice(1), old: o++ , file });
    else if (raw.startsWith('\\')) out.push({ type: 'meta', text: raw });
    else if (raw !== '' || o || n) out.push({ type: 'ctx', text: raw.slice(1), old: o++, new: n++ , file });
  }
  // trailing empty line after split
  while (out.length && out[out.length - 1].type === 'ctx' && out[out.length - 1].text === '') out.pop();
  return out;
}

const ROW: Record<Line['type'], string> = {
  add: 'bg-add text-add-fg',
  del: 'bg-del text-del-fg',
  ctx: '',
  hunk: 'bg-sunken text-muted',
  meta: 'text-muted italic',
};

/**
 * path is for syntax highlighting: the old and new sides are highlighted
 * separately (context + removed / context + added) so the grammar
 * sees coherent code, then the tokens are mapped back to lines.
 */
export function DiffView({ diff, maxHeight, path }: { diff: string; maxHeight?: string; path?: string }) {
  const lines = useMemo(() => parse(diff), [diff]);
  const tokens = useDiffTokens(lines, path);
  if (!diff.trim()) return <div className="px-3 py-4 text-sm text-muted">{t('diff.none')}</div>;
  return (
    <div className="overflow-auto font-mono text-[length:var(--code-size,12px)] leading-[1.55] [tab-size:var(--code-tab,2)]" style={{ maxHeight }}>
      <table className="min-w-full border-collapse">
        <tbody>
          {lines.map((l, i) => (
            <tr key={i} className={ROW[l.type]}>
              {l.type === 'hunk' || l.type === 'meta' ? (
                <td colSpan={3} className="px-3 py-0.5 whitespace-pre">
                  {l.text}
                </td>
              ) : (
                <>
                  <td className="w-px select-none px-1.5 text-right text-muted/70">{l.old ?? ''}</td>
                  <td className="w-px select-none px-1.5 text-right text-muted/70">{l.new ?? ''}</td>
                  <td className="pr-4 pl-1 whitespace-pre">
                    <span className="mr-1 select-none opacity-60">{l.type === 'add' ? '+' : l.type === 'del' ? '−' : ' '}</span>
                    <CodeLine tokens={tokens?.[i]} text={l.text} />
                  </td>
                </>
              )}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/**
 * Diff highlighting: for each file the old and new sides are highlighted
 * separately (context + removed / context + added) so the grammar
 * sees coherent code, then the tokens are mapped back to lines.
 */
function useDiffTokens(lines: Line[], path?: string): (Token[] | undefined)[] | null {
  const [tokens, setTokens] = useState<(Token[] | undefined)[] | null>(null);
  useEffect(() => {
    setTokens(null);
    const groups = new Map<string, { old: number[]; new: number[] }>();
    lines.forEach((l, i) => {
      const f = l.file ?? path;
      if (!f || l.type === 'hunk' || l.type === 'meta') return;
      const g = groups.get(f) ?? { old: [], new: [] };
      groups.set(f, g);
      if (l.type !== 'add') g.old.push(i);
      if (l.type !== 'del') g.new.push(i);
    });
    if (!groups.size) return;
    let alive = true;
    const out: (Token[] | undefined)[] = [];
    void Promise.all(
      [...groups].flatMap(([f, g]) =>
        (['old', 'new'] as const).map(async (side) => {
          const idx = g[side];
          const res = await tokenize(idx.map((i) => lines[i]!.text).join('\n'), { path: f }).catch(() => null);
          if (!res) return;
          idx.forEach((lineIdx, k) => {
            const kind = lines[lineIdx]!.type;
            // context comes from the new side, removed lines from the old one
            if ((side === 'old' && kind === 'del') || (side === 'new' && kind !== 'del')) out[lineIdx] = res[k];
          });
        }),
      ),
    ).then(() => alive && setTokens(out));
    return () => {
      alive = false;
    };
  }, [lines, path]);
  return tokens;
}

/** Diff for the Edit tool: old fragment → new one. */
export function editToDiff(oldStr: string, newStr: string): string {
  const o = oldStr.split('\n');
  const n = newStr.split('\n');
  return [`@@ -1,${o.length} +1,${n.length} @@`, ...o.map((l) => '-' + l), ...n.map((l) => '+' + l)].join('\n');
}
