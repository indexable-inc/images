# site

The `site/` domain is the marketing/changelog website for the `index` repo: a
SvelteKit 2 + Svelte 5 app that is fully prerendered to static HTML by
`@sveltejs/adapter-static` and published to GitHub Pages. It is content-first: a
flat, filterable "log" of short updates (one `.svx` file per entry) plus a hero
intro, per-entry permalink pages, and an RSS feed. There is no server runtime;
every route is computed at build time.

The site is a single JavaScript package (`site/package.json` name `ix-site`,
`site/package.json:2`); it is not a Rust crate and not a Nix-only package. It is
packaged for the flake by the shared Nix builder
`ix.buildSvelteSite` (`lib/build/svelte-site.nix`; see
[build-deploy](build-deploy/overview.md)) and exposed as `nix run .#site` / `nix build .#site`.
Read this page first, then the component pages it links.

## Units

| unit | kind | role |
| --- | --- | --- |
| `site/src/routes` | SvelteKit routes | page/endpoint tree: home feed, permalink, RSS, layout. See [routes](routes/overview.md). |
| `site/src/lib` | shared TS + Svelte | content data layer (`updates.ts`), components, the tag filter, diagrams, styling. See [lib](lib/overview.md). |
| `site/mdsvex.config.js` | preprocessor config | `.svx` -> Svelte: shiki dual-theme highlighting + a link-sanitizing rehype plugin. Covered in [lib](lib/overview.md). |
| `site/src/lib/updates/*.svx` | content | 27 changelog entries; markdown body + YAML frontmatter, optionally embedding diagram components. |
| flake build/deploy | Nix + CI | `ix.buildSvelteSite` in `lib/per-system.nix:427`, `.#site` output, GitHub Pages via `.github/workflows/pages.yml`. See [build-deploy](build-deploy/overview.md). |

## How it fits together

```
site/src/lib/updates/*.svx  (frontmatter + markdown + optional <script>)
   -> mdsvex (svelte.config.js:11) compiles each .svx to a Svelte Component
   -> updates.ts import.meta.glob (eager) collects them into siteUpdates[]
        (sorted newest-first, tags lowercased)
   -> routes read siteUpdates:
        /            +page.svelte   feed: FilterBar + UpdateEntry list
        /[id]        +page.svelte   one entry (prerendered per id)
        /feed.xml    +server.ts     RSS 2.0 over the same data
   -> adapter-static prerenders every route to build/ (HTML + assets)
   -> nix build .#site -> result/share/ix-site -> GitHub Pages (/index)
```

- **Content is the source of truth.** Each entry lives in one `.svx` file. Its
  frontmatter (`id`, `postedAt`, `title`, `tags`, `links`) is parsed by mdsvex
  and normalized in `updates.ts:41`; the compiled component is its rendered body.
  Adding an entry is adding a file; nothing else is registered by hand.
- **Everything prerenders.** `+layout.ts:1` sets `prerender = true` for the whole
  tree; `[id]/+page.ts` enumerates entries via `entries` (`[id]/+page.ts:7`) so
  every permalink is emitted; `feed.xml/+server.ts:11` prerenders the feed. The
  static adapter (`svelte.config.js:14`) writes `build/` with a `404.html`
  fallback.
- **Base path is `/index`.** GitHub project pages serve under a repo subpath, so
  `svelte.config.js:24` sets `paths.base = process.env.BASE_PATH ?? '/index'`.
  In-app links use `resolve()` from `$app/paths` so the prefix is applied once.
- **Dual-theme, no JS for color.** shiki emits both light and dark colors per
  span as CSS variables (`mdsvex.config.js:10`); `app.css` picks one with
  `prefers-color-scheme`. The whole UI is plain CSS variables, no framework.
- **Hydration-safe time.** Prerendered HTML renders dates in UTC; after mount
  each page re-renders `<time>` in the visitor's zone (`+page.svelte:11`,
  `format-posted-at.ts`).

## Invariants

- **One entry = one `.svx` file** under `site/src/lib/updates/`. `id` must be a
  lowercase slug, `postedAt` an ISO 8601 timestamp with offset, link hrefs
  absolute `https://` (enforced by `updates.test.ts`).
- **No raw HTML injection.** Titles go through `inlineTitleHtml` (escape then
  re-wrap backtick runs, `updates.ts:73`); body links are sanitized by the
  rehype plugin `safeHref` (`mdsvex.config.js:34`), which drops
  `javascript:`/protocol-relative URLs and keeps only `https`/`http`/`mailto`/
  root/fragment hrefs.
- **The default front-page filter is `interesting`** (`+page.svelte:18`); only
  entries tagged `interesting` show until the visitor edits or clears the filter
  expression.
- **Local preview == deploy.** `nix run .#site` serves the same `/index` build
  Pages deploys, via a miniserve `--route-prefix /index` wrapper
  (`lib/per-system.nix:432`), not a separate `BASE_PATH` rebuild.

## Glossary

- **update / entry**: one changelog post; a `.svx` file in
  `site/src/lib/updates/` with frontmatter + markdown body.
- **`.svx`**: an mdsvex file - markdown that may contain a Svelte `<script>` and
  components; compiled to a Svelte component by the mdsvex preprocessor.
- **mdsvex**: the markdown-in-Svelte preprocessor wired in `svelte.config.js:11`
  and configured by `mdsvex.config.js`.
- **filter expression**: the flecs-style boolean tag query on the home page
  (`nix & (rust | zig)`), parsed by `filter-expression.ts`.
- **frontmatter**: the YAML block at the top of each `.svx`; becomes
  `metadata` and is normalized to `SiteUpdateMeta` (`updates.ts:8`).
- **adapter-static**: the SvelteKit adapter that prerenders the app to flat
  files (`svelte.config.js:14`); output lands in `build/`.
- **base path**: the `/index` URL prefix for GitHub project pages
  (`svelte.config.js:24`).
- **shiki dual-theme**: syntax highlighting that ships both light and dark colors
  per token as CSS variables (`mdsvex.config.js:10`).

## Components

| component | page | what |
| --- | --- | --- |
| routes | [routes/overview.md](routes/overview.md) | the page/endpoint tree: layout, home feed, `[id]` permalink, `feed.xml`, prerender + base path |
| lib | [lib/overview.md](lib/overview.md) | content data layer, components (`UpdateEntry`, `FilterBar`), the tag filter, xyflow diagrams, mdsvex, styling |
| build-deploy | [build-deploy/overview.md](build-deploy/overview.md) | recipe: `ix.buildSvelteSite`, `.#site`/`.#site-dev`, vitest checks, GitHub Pages deploy |
