import { memo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkBreaks from 'remark-breaks';
import { CodeBlock } from './Code';

/**
 * breaks: a single line break = a new line (as in GitHub comments
 * and in Claude/ChatGPT chats): agents often write lists without markers. For
 * files (README) it is off, as on GitHub.
 */
export const Markdown = memo(function Markdown({ text, breaks = false }: { text: string; breaks?: boolean }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={breaks ? [remarkGfm, remarkBreaks] : [remarkGfm]}
        components={{
          a: ({ node: _n, ...props }) => <a {...props} target="_blank" rel="noreferrer" />,
          // code block: ```lang → highlighting; inline code is left as is
          pre: ({ node }) => {
            const code = node?.children[0];
            if (!code || code.type !== 'element' || code.tagName !== 'code') return <pre />;
            const cls = (code.properties?.className as string[] | undefined) ?? [];
            const lang = cls.find((c) => c.startsWith('language-'))?.slice(9) ?? null;
            const text = code.children.map((c) => (c.type === 'text' ? c.value : '')).join('');
            return <CodeBlock code={text} lang={lang} />;
          },
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
