# Artifact

A single-page Svelte artifact with versioned content.

```sh
bun install
bun dev
```

Content lives in `src/versions/v*.svx` (markdown + Svelte via mdsvex).
Each version is one immutable file; the header picker switches between
them and `diff` shows a line diff of any two. To publish a new
revision, copy the highest `v<n>.svx` to `v<n+1>.svx` and edit that.

Components under `src/lib/components/` (Callout, Figure, DiffView,
VersionPicker, ThemeToggle) and the theme tokens in `src/app.css` are
yours to extend.
