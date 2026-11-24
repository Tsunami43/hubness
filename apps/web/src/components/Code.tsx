import { useEffect, useState } from 'react';
import { tokenize, tokenStyle, type Token } from '../lib/highlight';

/** Highlighting tokens for code; null while the grammar is loading (or the language is unknown). */
export function useTokens(code: string, source: { path: string } | { lang: string } | null): Token[][] | null {
  const [tokens, setTokens] = useState<Token[][] | null>(null);
  const key = source ? ('path' in source ? `p:${source.path}` : `l:${source.lang}`) : '';
  useEffect(() => {
    setTokens(null);
    if (!source || !code) return;
    let alive = true;
    tokenize(code, source).then(
      (t) => alive && setTokens(t),
      () => {},
    );
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code, key]);
  return tokens;
}

/** A line of code: highlighted if there are tokens, otherwise as is. */
export function CodeLine({ tokens, text }: { tokens?: Token[]; text: string }) {
  if (!tokens) return <>{text}</>;
  return (
    <>
      {tokens.map((t, i) => (
        <span key={i} style={tokenStyle(t)}>
          {t.content}
        </span>
      ))}
    </>
  );
}

/** Code block from Markdown (```lang). */
export function CodeBlock({ code, lang }: { code: string; lang: string | null }) {
  const text = code.replace(/\n$/, '');
  const tokens = useTokens(text, lang ? { lang } : null);
  const lines = text.split('\n');
  return (
    <pre>
      <code>
        {lines.map((l, i) => (
          <span key={i}>
            <CodeLine tokens={tokens?.[i]} text={l} />
            {i < lines.length - 1 ? '\n' : null}
          </span>
        ))}
      </code>
    </pre>
  );
}

/** File contents (e.g. an agent's Write) highlighted by file name. */
export function CodeFile({ code, path }: { code: string; path?: string }) {
  const text = code.replace(/\n$/, '');
  const tokens = useTokens(text, path ? { path } : null);
  const lines = text.split('\n');
  return (
    <pre className="!my-0">
      <code>
        {lines.map((l, i) => (
          <span key={i}>
            <CodeLine tokens={tokens?.[i]} text={l} />
            {i < lines.length - 1 ? '\n' : null}
          </span>
        ))}
      </code>
    </pre>
  );
}
