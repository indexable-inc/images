// shiki is loaded at runtime from esm.sh (lazily, only when the feed first shows
// code) so the committed single-file artifact stays small. Declare the one entry
// point the dashboard uses so the TypeScript build resolves the URL import.
declare module 'https://esm.sh/shiki@4' {
  // Render code to themed HTML. We pass a dual-theme map and `defaultColor: false`
  // so each token carries both palettes as CSS vars (see highlight.ts).
  export function codeToHtml(
    code: string,
    options: {
      lang: string;
      themes: { light: string; dark: string };
      defaultColor?: boolean | string;
    },
  ): Promise<string>;
}
