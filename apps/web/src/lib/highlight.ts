import type { HighlighterCore, ThemedToken, ThemeRegistrationRaw } from 'shiki/core';

/**
 * Syntax highlighting (GitHub-style): TextMate grammars via Shiki —
 * the same ones as in VS Code. The regex engine is JS (no WASM). Language grammars
 * are loaded on demand. Colors come not from a Shiki theme but from the app theme's
 * --syntax-* CSS variables (theme/index.ts); the scope → role mapping
 * follows the GitHub theme.
 */

const v = (role: string) => `var(--syntax-${role})`;
const FG = 'var(--color-fg)';

const THEME: ThemeRegistrationRaw = {
  name: 'hubness',
  type: 'dark',
  colors: { 'editor.foreground': FG, 'editor.background': 'transparent' },
  // rules go in settings (Shiki uses them instead of tokenColors when the field is present)
  settings: [
    { settings: { foreground: FG, background: 'transparent' } },
    { scope: ['comment', 'punctuation.definition.comment', 'string.comment'], settings: { foreground: v('comment') } },
    { scope: ['constant.other.placeholder', 'constant.character'], settings: { foreground: v('keyword') } },
    {
      scope: ['constant', 'entity.name.constant', 'variable.other.constant', 'variable.other.enummember', 'variable.language', 'entity'],
      settings: { foreground: v('constant') },
    },
    { scope: ['entity.name', 'meta.export.default', 'meta.definition.variable'], settings: { foreground: v('entity') } },
    {
      scope: ['variable.parameter.function', 'meta.jsx.children', 'meta.block', 'meta.tag.attributes', 'meta.object.member', 'meta.embedded.expression'],
      settings: { foreground: FG },
    },
    { scope: ['entity.name.function', 'meta.function-call.generic', 'variable.function', 'support.function'], settings: { foreground: v('function') } },
    { scope: ['entity.name.tag', 'support.class.component'], settings: { foreground: v('tag') } },
    { scope: ['keyword', 'storage', 'storage.type'], settings: { foreground: v('keyword') } },
    { scope: ['storage.modifier.package', 'storage.modifier.import', 'storage.type.java'], settings: { foreground: FG } },
    { scope: ['string', 'string punctuation.section.embedded source'], settings: { foreground: v('string') } },
    { scope: ['support', 'meta.property-name', 'support.constant', 'support.variable', 'meta.module-reference'], settings: { foreground: v('constant') } },
    { scope: 'variable', settings: { foreground: v('entity') } },
    { scope: 'variable.other', settings: { foreground: FG } },
    { scope: ['invalid.broken', 'invalid.deprecated', 'invalid.illegal', 'invalid.unimplemented', 'message.error'], settings: { foreground: 'var(--color-danger)', fontStyle: 'italic' } },
    { scope: 'string variable', settings: { foreground: v('constant') } },
    { scope: ['source.regexp', 'string.regexp'], settings: { foreground: v('string') } },
    { scope: 'string.regexp constant.character.escape', settings: { foreground: v('tag'), fontStyle: 'bold' } },
    { scope: 'support.type.property-name.json', settings: { foreground: v('tag') } },
    { scope: 'punctuation.definition.list.begin.markdown', settings: { foreground: v('entity') } },
    { scope: ['markup.heading', 'markup.heading entity.name'], settings: { foreground: v('constant'), fontStyle: 'bold' } },
    { scope: 'markup.quote', settings: { foreground: v('tag') } },
    { scope: 'markup.italic', settings: { fontStyle: 'italic' } },
    { scope: 'markup.bold', settings: { fontStyle: 'bold' } },
    { scope: 'markup.inline.raw', settings: { foreground: v('constant') } },
    { scope: ['markup.deleted', 'meta.diff.header.from-file', 'punctuation.definition.deleted'], settings: { foreground: 'var(--color-del-fg)' } },
    { scope: ['markup.inserted', 'meta.diff.header.to-file', 'punctuation.definition.inserted'], settings: { foreground: 'var(--color-add-fg)' } },
    { scope: ['markup.changed', 'punctuation.definition.changed'], settings: { foreground: v('entity') } },
    { scope: 'punctuation.section.embedded', settings: { foreground: v('keyword') } },
    { scope: 'meta.diff.range', settings: { foreground: v('function'), fontStyle: 'bold' } },
    { scope: 'meta.diff.header', settings: { foreground: v('constant') } },
    { scope: ['constant.other.reference.link', 'string.other.link'], settings: { foreground: v('string') } },
  ],
};

// extensions and file names missing from Shiki's aliases
const EXTRA: Record<string, string> = {
  mjs: 'javascript',
  cjs: 'javascript',
  mts: 'typescript',
  cts: 'typescript',
  h: 'c',
  hh: 'cpp',
  hpp: 'cpp',
  cc: 'cpp',
  cxx: 'cpp',
  kts: 'kotlin',
  gradle: 'groovy',
  svg: 'xml',
  plist: 'xml',
  csproj: 'xml',
  htm: 'html',
  yml: 'yaml',
  lock: 'yaml',
  env: 'dotenv',
  gql: 'graphql',
  tf: 'terraform',
  tfvars: 'terraform',
  ps1: 'powershell',
  psm1: 'powershell',
  bash: 'shellscript',
  zsh: 'shellscript',
  sh: 'shellscript',
  fish: 'fish',
  ex: 'elixir',
  exs: 'elixir',
  erl: 'erlang',
  hs: 'haskell',
  jl: 'julia',
  pl: 'perl',
  rs: 'rust',
  mdx: 'mdx',
  txt: '',
};
const NAMES: Record<string, string> = {
  dockerfile: 'docker',
  containerfile: 'docker',
  makefile: 'make',
  gnumakefile: 'make',
  justfile: 'just',
  '.gitignore': 'shellscript',
  '.dockerignore': 'shellscript',
  '.env': 'dotenv',
  '.bashrc': 'shellscript',
  '.zshrc': 'shellscript',
  '.profile': 'shellscript',
  'cargo.lock': 'toml',
  'go.mod': 'go',
  'go.sum': '',
  'pnpm-lock.yaml': 'yaml',
  'package-lock.json': 'json',
  'tsconfig.json': 'jsonc',
  '.editorconfig': 'ini',
};

type Langs = typeof import('shiki/langs');
let langs: Langs | null = null;
let aliases: Map<string, string> | null = null;
let core: Promise<HighlighterCore> | null = null;

async function setup() {
  if (!core)
    core = (async () => {
      const [{ createHighlighterCore }, { createJavaScriptRegexEngine }, l] = await Promise.all([
        import('shiki/core'),
        import('shiki/engine/javascript'),
        import('shiki/langs'),
      ]);
      langs = l;
      aliases = new Map();
      for (const info of l.bundledLanguagesInfo) {
        aliases.set(info.id, info.id);
        for (const a of info.aliases ?? []) aliases.set(a, info.id);
      }
      return createHighlighterCore({ themes: [THEME], langs: [], engine: createJavaScriptRegexEngine({ forgiving: true }) });
    })();
  return core;
}

/** Language by file name: name (Dockerfile, Makefile) → compound/plain extension. */
function resolveLang(path: string): string | null {
  const name = path.slice(path.lastIndexOf('/') + 1).toLowerCase();
  if (name in NAMES) return NAMES[name] || null;
  if (name.startsWith('.env')) return 'dotenv';
  if (name.startsWith('dockerfile')) return 'docker';
  const dot = name.lastIndexOf('.');
  if (dot < 0) return null;
  const ext = name.slice(dot + 1);
  if (ext in EXTRA) return EXTRA[ext] || null;
  return aliases?.get(ext) ?? null;
}

/** Language for code from Markdown (```ts) — by alias. */
function resolveFence(lang: string): string | null {
  const l = lang.toLowerCase();
  return EXTRA[l] || aliases?.get(l) || null;
}

export type Token = Pick<ThemedToken, 'content' | 'color' | 'fontStyle'>;

const MAX_CHARS = 300_000;
const MAX_LINES = 10_000;

/**
 * Tokens per line. source is a file path or a language from Markdown.
 * null — unknown language or the file is too large (shown without color).
 */
export async function tokenize(code: string, source: { path: string } | { lang: string }): Promise<Token[][] | null> {
  if (code.length > MAX_CHARS) return null;
  const h = await setup();
  const id = 'path' in source ? resolveLang(source.path) : resolveFence(source.lang);
  if (!id || !langs) return null;
  const loader = langs.bundledLanguages[id as keyof Langs['bundledLanguages']];
  if (!loader) return null;
  if (!h.getLoadedLanguages().includes(id)) await h.loadLanguage(loader);
  const lines = h.codeToTokensBase(code, { lang: id, theme: 'hubness' });
  return lines.length > MAX_LINES ? null : lines;
}

/** Token style: the color is a theme CSS variable. */
export function tokenStyle(t: Token): React.CSSProperties | undefined {
  const fs = t.fontStyle ?? 0;
  if (!t.color && !fs) return undefined;
  return {
    color: t.color && t.color !== FG ? t.color : undefined,
    fontStyle: fs & 1 ? 'italic' : undefined,
    fontWeight: fs & 2 ? 600 : undefined,
    textDecoration: fs & 4 ? 'underline' : undefined,
  };
}
