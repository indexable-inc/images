// Ambient type for the `$islands-theme` alias (vite.config.js): the JetBrains
// Islands palette JSON owned by packages/code/code-highlight. Non-color keys
// (the `_comment` header) are absent from the variant tables.
declare module '$islands-theme' {
  const palette: {
    dark: Record<string, string>;
    light: Record<string, string>;
  };
  export default palette;
}
