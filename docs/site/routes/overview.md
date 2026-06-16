# routes

`site/src/routes` is the SvelteKit route tree. It is small: a root layout, a home
feed, a dynamic permalink, and one non-HTML endpoint (the RSS feed). Every route
is prerendered to static files (no server runtime). All page data comes from
[`$lib/updates`](../lib/overview.md); routes hold presentation and prerender
config only.

## App shell

The HTML template and global config live next to the routes under `site/src`:

- `src/app.html` - the document shell. `%sveltekit.head%`/`%sveltekit.body%`
  placeholders, `data-sveltekit-preload-data="hover"` (prefetch on hover), and
  the favicon at `%sveltekit.assets%/favicon.svg` (`src/app.html:5,9`). The only
  static asset is `static/favicon.svg`.
- `src/app.css` - the global stylesheet, imported once by the layout. Theme
  tokens and component styles; see styling in [lib](../lib/overview.md).
- `src/app.d.ts` - the empty SvelteKit `App` namespace (no custom `Locals` etc.).
- `src/mdsvex.d.ts` - ambient module decl for `*.svx` (default `Component` +
  `metadata`).

## Prerender + base path (tree-wide)

- `routes/+layout.ts:1` - `export const prerender = true;`. This applies to the
  whole route subtree, so the entire app is static.
- `svelte.config.js:24` sets `paths.base = process.env.BASE_PATH ?? '/index'`.
  Code never hardcodes `/index`; it calls `resolve()` from `$app/paths`
  (`+layout.svelte:3`, `UpdateEntry.svelte:2`) so the base is applied in one
  place. Override `BASE_PATH=""` for apex/custom-domain deploys.

## `+layout.svelte` (`routes/+layout.svelte`)

The chrome wrapped around every page.

- Imports `app.css` (`:5`) so global styles load once.
- `<svelte:head>` adds the RSS `<link rel="alternate" type="application/rss+xml">`
  pointing at `siteFeedUrl` (`:13-15`).
- `<header>`: wordmark link to home (`resolve('/')`, `:9,18`) and a `<nav>` with
  three external/site links: GitHub repo, `https://ix.dev`, and the RSS feed
  (`resolve('/feed.xml')`, `:19-23`).
- `<main>{@render children()}</main>` renders the active page (Svelte 5 snippet
  prop, `:7,26-28`).

## `/` home feed (`routes/+page.svelte`)

The landing page: a hero plus the filterable update log.

- State (Svelte 5 runes):
  - `timeZone` resolved in `onMount` (`:11-14`); SSR renders UTC, hydration
    switches `<time>` to the visitor's zone.
  - `filter` initialized to `'interesting'` (`:18`) - the default narrows the log
    to author-flagged headline items.
  - `parsed = parseFilter(filter)` and `filtered` derived from it (`:20-23`);
    when the expression is invalid, the full list is shown and `error` is set.
- `<svelte:head>` sets `<title>` and the `description` meta from `siteIntro`
  (`:27-30`).
- Renders `siteIntro` in a `.hero`, then [`FilterBar`](../lib/overview.md) (value,
  `onChange`, match/total counts, error), then an ordered list of
  [`UpdateEntry`](../lib/overview.md) keyed by `update.id` (`:47-53`).

## `/[id]` permalink (`routes/[id]/`)

One page per update, addressable by slug.

- `[id]/+page.ts`:
  - `prerender = true` (`:5`).
  - `entries` (`:7-8`) returns `{ id }` for every entry in `siteUpdates`, so the
    static adapter emits one HTML file per update.
  - `load` (`:10-13`) finds the entry by `params.id`; an unknown id throws
    `error(404, ...)`, which adapter-static renders against the `404.html`
    fallback.
- `[id]/+page.svelte`:
  - Same `timeZone` onMount pattern (`:9-12`).
  - `titleText = plainText(data.update.title)` feeds `<title>`/description
    (`:14,17-20`).
  - Renders one [`UpdateEntry`](../lib/overview.md) with `titleTag="h1"` (the
    feed uses the default `h2`).

## `/feed.xml` RSS endpoint (`routes/feed.xml/+server.ts`)

A non-HTML route: a `+server.ts` `GET` that returns an RSS 2.0 document.

- `prerender = true` (`:11`) so the feed is emitted as a static `feed.xml`.
- `GET()` (`:50-72`) builds `<channel>` metadata from `siteUrl`/`siteFeedUrl`/
  `siteIntro`, `lastBuildDate` from the newest entry, and one `<item>` per update
  via `itemXml` (`:36-48`).
- Item bodies use `updateScript(update)` (title + flattened body,
  `updates.ts:90`); all text is run through a local `escapeXml` (`:13-30`) and
  dates through `rssDate` -> `toUTCString` (`:32-34`).
- Returns `content-type: application/rss+xml; charset=utf-8` (`:67-71`).

## Notes

- There is no error page component (`+error.svelte`) or non-prerendered route;
  the static `404.html` fallback (`svelte.config.js:17`) handles unknown paths.
- All in-app navigation is base-path-aware via `resolve()`; external links in the
  header and in entry frontmatter are absolute URLs.
