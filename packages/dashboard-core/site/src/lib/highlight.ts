// Syntax highlighting for the feed's code column, via Shiki loaded lazily from
// esm.sh — the same runtime-CDN pattern the data layer uses for loro-crdt, so the
// committed single-file artifact stays small and shiki is fetched once, only when
// code is first shown. Output is dual-theme (github-light + github-dark): every
// token carries both palettes as CSS vars and the column picks one from the OS
// color-scheme (see the `.shiki` rules in style.css). Precedent: margins-ink/site
// highlights with shiki the same way (at build time there; at runtime here).
//
// shiki@4 on esm.sh resolves to the JavaScript regex engine (no oniguruma wasm),
// so this is a plain JS download with no extra asset.

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type CodeToHtml = (code: string, options: any) => Promise<string>;

let loading: Promise<CodeToHtml> | null = null;

function load(): Promise<CodeToHtml> {
  loading ??= import('https://esm.sh/shiki@4').then(
    (m) => (m as { codeToHtml: CodeToHtml }).codeToHtml,
  );
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
      themes: { light: 'github-light', dark: 'github-dark' },
      defaultColor: false,
    });
  } catch {
    return null;
  }
}
