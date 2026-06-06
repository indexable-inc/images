// Markdown rendering for assistant messages.
//
// We disable HTML and raw inline HTML to keep the surface safe by
// default; the server is internal but room messages can carry whatever
// the agent and tools dump out, and we never want a stray <script>.
//
// Fenced code blocks go through Shiki (TextMate grammars; same engine
// VS Code uses) using the github-dark theme to match the existing
// DiffBlock styling. Shiki loads lazily — until the highlighter is
// ready, fences render as plain escaped <pre><code>; once ready, the
// `ready` store flips, MarkdownBody re-derives, and the upgraded HTML
// replaces the plain version in place.

import MarkdownIt from 'markdown-it';
import { writable, type Readable } from 'svelte/store';
import type { BundledLanguage, BundledTheme, HighlighterGeneric } from 'shiki';

// Dual-theme: Shiki bakes BOTH theme colors into the output as
// `--shiki-light` and `--shiki-dark` CSS variables on every token
// span. app.css picks which variable to use via prefers-color-scheme,
// so switching themes is free at runtime (no JS, no flash). Same
// pattern the iam blog uses.
const THEMES = { light: 'github-light', dark: 'github-dark' } as const;
const LANGS = [
  'typescript',
  'javascript',
  'tsx',
  'jsx',
  'svelte',
  'rust',
  'python',
  'go',
  'json',
  'yaml',
  'toml',
  'bash',
  'shellscript',
  'sql',
  'html',
  'css',
  'scss',
  'markdown',
  'diff',
  'dockerfile',
  'nix',
  'ruby',
  'java',
  'kotlin',
  'swift',
  'c',
  'cpp',
  'csharp',
  'php',
  // Special shiki language that interprets ANSI escape sequences as
  // colored spans. Used by ShellWork to render terminal output with
  // colors preserved instead of as raw bytes.
  'ansi'
] as const;

let highlighter: HighlighterGeneric<BundledLanguage, BundledTheme> | null = null;
const _ready = writable(false);
export const highlighterReady: Readable<boolean> = { subscribe: _ready.subscribe };

// Same source string renders to the same HTML, so cache the result.
// This is the hot path on every thread switch / message update —
// without the cache we re-parse every message in the transcript on
// each render. Declared before the async highlighter loader so the
// loader can clear the cache once highlighting becomes available.
const renderCache = new Map<string, string>();
const RENDER_CACHE_MAX = 1024;

// Kick off highlighter load on module import. First markdown render
// just escapes; once this resolves we flip `ready` so MarkdownBody
// re-derives with proper highlighting.
void (async () => {
  try {
    const { createHighlighter } = await import('shiki');
    highlighter = await createHighlighter({
      themes: [THEMES.light, THEMES.dark],
      langs: [...LANGS]
    });
    // Highlighting changes output for the same source — clear the
    // cache so the upgraded HTML actually appears on re-render.
    renderCache.clear();
    _ready.set(true);
  } catch (err) {
    console.warn('room: shiki highlighter failed to load', err);
  }
})();

function resolveLang(raw: string): string | null {
  const lang = raw.trim().toLowerCase();
  if (!lang) return null;
  // Common aliases users actually type in fences. Shiki accepts most
  // of these directly but normalizing keeps the cache effective.
  const aliasMap: Record<string, string> = {
    sh: 'bash',
    shell: 'bash',
    zsh: 'bash',
    ts: 'typescript',
    js: 'javascript',
    py: 'python',
    rs: 'rust',
    yml: 'yaml',
    md: 'markdown',
    dockerfile: 'dockerfile'
  };
  const normalized = aliasMap[lang] ?? lang;
  return (LANGS as readonly string[]).includes(normalized) ? normalized : null;
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

const md = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: false,
  typographer: false,
  highlight(code: string, lang: string): string {
    if (!highlighter) {
      // Plain fallback while shiki is still loading. The wrapper here
      // matches Shiki's output shape so swapping in won't reflow.
      return `<pre class="shiki" tabindex="0"><code>${escapeHtml(code)}</code></pre>`;
    }
    const resolved = resolveLang(lang);
    try {
      // `defaultColor: false` makes Shiki emit BOTH theme colors as
      // CSS custom properties (--shiki-light / --shiki-dark) on every
      // token span. app.css picks which one to use based on the
      // active color scheme, so theme switching is a CSS toggle with
      // zero re-render. Same approach the iam + margins-ink sites use.
      return highlighter.codeToHtml(code, {
        lang: resolved ?? 'text',
        themes: { light: THEMES.light, dark: THEMES.dark },
        defaultColor: false
      });
    } catch {
      return `<pre class="shiki" tabindex="0"><code>${escapeHtml(code)}</code></pre>`;
    }
  }
});

// Always open links in a new tab.
const defaultRender =
  md.renderer.rules.link_open ??
  function (tokens, idx, options, _env, self) {
    return self.renderToken(tokens, idx, options);
  };
md.renderer.rules.link_open = (tokens, idx, options, env, self) => {
  const token = tokens[idx]!;
  token.attrSet('target', '_blank');
  token.attrSet('rel', 'noreferrer noopener');
  return defaultRender(tokens, idx, options, env, self);
};

// Highlight a code string and return just the inner token spans, with
// no <pre>/<code> wrappers. Lets the caller drop syntax-highlighted
// code into arbitrary inline contexts (e.g. the Bash row in ToolWork).
// Pairs with the `.shiki-inline` rule in app.css that paints the
// per-token CSS variables Shiki emits.
const CODE_INNER_RE = /<code[^>]*>([\s\S]*?)<\/code>/;
export function highlightInline(code: string, lang: string): string {
  if (!highlighter) return escapeHtml(code);
  const resolved = resolveLang(lang) ?? 'text';
  try {
    const html = highlighter.codeToHtml(code, {
      lang: resolved,
      themes: { light: THEMES.light, dark: THEMES.dark },
      defaultColor: false
    });
    const m = CODE_INNER_RE.exec(html);
    return m ? m[1]! : escapeHtml(code);
  } catch {
    return escapeHtml(code);
  }
}

export function renderMarkdown(input: string): string {
  const hit = renderCache.get(input);
  if (hit !== undefined) return hit;
  const html = md.render(input);
  if (renderCache.size >= RENDER_CACHE_MAX) {
    // Map preserves insertion order — dropping the first key evicts the
    // oldest entry. Good-enough LRU approximation for this volume.
    const oldest = renderCache.keys().next().value;
    if (oldest !== undefined) renderCache.delete(oldest);
  }
  renderCache.set(input, html);
  return html;
}
