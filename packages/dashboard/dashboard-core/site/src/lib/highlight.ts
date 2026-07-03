// Syntax highlighting for the feed's code column, via Shiki loaded lazily from
// esm.sh — the same runtime-CDN pattern the data layer uses for loro-crdt, so the
// committed single-file artifact stays small and shiki is fetched once, only when
// code is first shown. Output is dual-theme (islands light + dark, generated from
// the repo's JetBrains Islands palette — see $lib/theme/islands): every token
// carries both palettes as CSS vars and the column picks one from the OS
// color-scheme (see the `.shiki` rules in style.css). Precedent: margins-ink/site
// highlights with shiki the same way (at build time there; at runtime here).
//
// shiki@4 on esm.sh resolves to the JavaScript regex engine (no oniguruma wasm),
// so this is a plain JS download with no extra asset.
import { islandsDark, islandsLight } from './theme/islands';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type CodeToHtml = (code: string, options: any) => Promise<string>;

let loading: Promise<CodeToHtml> | null = null;
let attempts = 0;

function load(): Promise<CodeToHtml> {
  if (!loading) {
    // Retry after a failed load instead of caching the rejection forever (one
    // offline blip used to disable highlighting for the whole session). The
    // browser module map also memoizes a FAILED import by specifier, so a
    // retry needs a cache-busting query to actually re-fetch.
    const url = attempts++ ? `https://esm.sh/shiki@4?retry=${attempts}` : 'https://esm.sh/shiki@4';
    loading = import(url).then((m) => (m as { codeToHtml: CodeToHtml }).codeToHtml);
    loading.catch(() => {
      loading = null;
    });
  }
  return loading;
}

// Map the producer's language hint onto a shiki grammar id. shiki lazy-loads the
// grammar on demand; an unknown id throws and we fall back to plain text.
const ALIAS: Record<string, string> = {
  sh: 'bash',
  shell: 'bash',
  zsh: 'bash',
  console: 'bash',
  py: 'python',
  js: 'javascript',
  ts: 'typescript',
  rs: 'rust',
  yml: 'yaml',
};

// Highlight `code` as `lang`, returning shiki HTML, or null when there is nothing
// to highlight / the grammar is unknown / the CDN is unreachable. The caller then
// renders the raw text instead. shiki escapes token text, so the HTML is safe to
// inject; null keeps the no-highlight path on a plain text binding.
export async function highlight(code: string, lang: string): Promise<string | null> {
  const language = ALIAS[lang] ?? lang;
  if (!code || !language || language === 'text' || language === 'plaintext') return null;
  try {
    const codeToHtml = await load();
    return await codeToHtml(code, {
      lang: language,
      themes: { light: islandsLight, dark: islandsDark },
      defaultColor: false,
    });
  } catch {
    return null;
  }
}

// Highlight `code` and return one HTML string per source line (shiki's `.line`
// spans), or null on the same fallbacks as `highlight`. For views that lay out
// their own gutter/rows (the inline trace, the file-view slice) rather than
// injecting the whole `<pre>`.
export async function highlightLines(code: string, lang: string): Promise<string[] | null> {
  const html = await highlight(code, lang);
  if (!html) return null;
  const tpl = document.createElement('template');
  tpl.innerHTML = html;
  const spans = tpl.content.querySelectorAll('.line');
  return spans.length ? Array.from(spans, (s) => s.outerHTML) : null;
}
