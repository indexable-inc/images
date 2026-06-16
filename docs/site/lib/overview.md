# lib

`site/src/lib` is the shared layer the [routes](../routes/overview.md) build on:
the content data layer that turns `.svx` files into `siteUpdates[]`, the
presentation components, the boolean tag filter, the global stylesheet, and the
mdsvex rendering pipeline. The xyflow diagram system is large enough to have its
own page: [diagrams](diagrams.md).

Imported via SvelteKit's `$lib` alias. vitest re-wires `$lib` and mdsvex by hand
(`vitest.config.ts:16-27`) because the browser test runner does not load the full
SvelteKit plugin.

## Content data layer (`updates.ts`)

The single source of truth for entries. Public surface (`updates.ts`):

- Types: `SiteUpdateLink` (`:3`), `SiteUpdateMeta` (`:8`: `id`, `postedAt`,
  `title`, `links`, `tags`), `SiteUpdate` (`:22`: meta + compiled `component` +
  `rawBody`).
- `siteUpdates: SiteUpdate[]` (`:41`) - the entry list. Built from two
  `import.meta.glob` calls over `./updates/*.svx`: one eager import of the
  compiled module (`:34`, gives `metadata` + default `Component`) and one
  `query: '?raw'` import of the source text (`:35-39`). For each file it spreads
  `metadata`, lowercases `tags` (`:44`), keeps the compiled `default` as
  `component`, and strips frontmatter from the raw text into `rawBody` (`:46`,
  `stripFrontmatter` `:55`). Sorted newest-first by `Date.parse(postedAt)`
  (`:48`). Adding an entry needs no registration; the glob discovers it.
- Constants: `siteUrl = 'https://indexable-inc.github.io/index/'` (`:50`),
  `siteFeedUrl = siteUrl + 'feed.xml'` (`:51`), `siteIntro` (`:52`).
- Text helpers:
  - `plainText(markdown)` (`:59`) - strips `<script>` blocks, capitalized
    component tags, fenced/inline code, bold/italic, and link syntax for plain
    text (titles in `<title>`, RSS bodies).
  - `inlineTitleHtml(markdown)` (`:73`) - HTML-escapes, then re-wraps backtick
    runs as `<code>`; the only sanctioned HTML for titles.
  - `updateScript(update)` (`:90`) - `plainText(title) + '. ' + plainText(body)`,
    used by the RSS `<description>`.
  - `updateUrl(id)` (`:96`) - absolute permalink for a slug (`siteUrl + id`).

`SvxModule.metadata.tags` is optional because mdsvex does not validate
frontmatter; the loader normalizes it to a required `string[]` (`:31,44`).

## Tag filter (`filter-expression.ts`)

A tiny flecs-style boolean tag query (`nix & (rust | zig)`), used by the home
feed. Precedence NOT > AND > OR; empty input matches everything (`:10,30`).

- `parseFilter(input): FilterExpression` (`:32`) - tokenizes (`tokenize` `:56`),
  recursive-descent parses (`Parser` `:102`: `parseOr`/`parseAnd`/`parseNot`/
  `parseAtom`), and returns either `{ ok: true, matches }` or `{ ok: false,
  error }`. `matches(tags)` evaluates the AST against a `Set` (`evaluate` `:216`).
  Adjacent terms are an implicit AND (`:139-140`).
- `highlightExpression(input): HighlightToken[]` (`:181`) - a tolerant tokenizer
  that never fails: every character lands in some span (`tag`/`op-*`/`paren`/
  `space`/`error`) so the rendered overlay matches the input character for
  character. Used by `FilterBar`. Covered by `filter-expression.test.ts`.

## Components

### `UpdateEntry.svelte`

Renders one entry, shared by the feed and permalink pages. Props (`:6-11`):
`update: SiteUpdate`, `timeZone`, `titleTag?: 'h1' | 'h2'` (default `h2`).

- `<time datetime={postedAt}>` labeled via `formatPostedAt` (`:17,22`).
- Title is `inlineTitleHtml(update.title)` rendered with `{@html}` (eslint
  disabled inline, `:25-32`); on the feed it is an `<a>` to the permalink
  (`resolve('/[id]', { id })`, `:18`), on a permalink page a bare `<h1>`.
- `<Body />` is the compiled `.svx` component (`update.component`, `:15,35`).
- Optional `.refs` (frontmatter `links`, `rel="external"`, `:37-44`) and `.tags`
  list (`:45-51`).

### `FilterBar.svelte`

The home-page filter input with live syntax highlighting. Props (`:4-10`):
`value`, `onChange`, `matchCount`, `totalCount`, `error?`.

- Technique: a transparent-text `<input>` (visible caret) layered over an
  `.overlay` that paints `highlightExpression(value)` tokens (`:14,32-53`).
  Padding/font/line-height match pixel-for-pixel (CSS comment `:98-101`).
- `syncScroll` (`:21-25`) mirrors the input's `scrollLeft` onto the overlay so
  long expressions stay aligned. `matchCount / totalCount` shown live; `error`
  rendered below when the expression fails to parse. Token colors are scoped
  CSS using `light-dark()` (`:143-166`).

### Time formatting (`format-posted-at.ts`)

`formatPostedAt(postedAt, zone)` (`:4`) renders `Mon D, YYYY . HH:MM TZ` via
`Intl.DateTimeFormat`. `zone` defaults to `'UTC'` so SSR output is identical for
every visitor; callers pass the resolved local zone after hydration. Both pages
follow this SSR-UTC / client-local pattern.

## Diagrams

The `lib/diagrams/` directory is an interactive flowchart system built on
`@xyflow/svelte`, embedded inside `.svx` bodies. It has its own page:
[diagrams](diagrams.md). Summary: `DiagramFrame.svelte` wraps `<SvelteFlow>`
(mount-gated, non-interactive inline, expandable modal with focus trap),
`BoxNode.svelte` is the one custom node type, and the `*Diagram.svelte` files are
thin data-only wrappers.

## Content entries (`lib/updates/*.svx`)

27 changelog entries plus `fixtures/mdsvex-safety.svx` (a test fixture). Each
entry is YAML frontmatter (`id`, `postedAt`, `title`, `tags`, `links`) followed
by markdown; a few open with a Svelte `<script>` that imports a diagram and embed
it inline (e.g. `updates/ix-mcp-python.svx:11-17`). Frontmatter is parsed by
mdsvex into `metadata`; the body compiles to the component rendered as `<Body />`.

## mdsvex pipeline (`mdsvex.config.js`)

`.svx` is registered in `svelte.config.js:8` and preprocessed by `mdsvex` with
`siteMdsvexOptions` (`svelte.config.js:11`). The config:

- `highlight.highlighter = highlightCode` (`:10-14`) - shiki dual-theme. Each
  code block renders with both `github-light` and `github-dark`, colors emitted
  as CSS variables (`defaultColor: false`, `:53`); `app.css` selects one per
  `prefers-color-scheme`. Unknown languages fall back to `text`
  (`isMissingShikiLanguage`, `:57`).
- `rehypePlugins: [sanitizeLinks]` (`:9`) - rewrites every `<a href>` in body
  content through `safeHref` (`:34`): keeps root (`/`) and fragment (`#`) hrefs,
  allows only `https`/`http`/`mailto` absolute URLs, and drops everything else
  (e.g. `javascript:`, protocol-relative `//`). A dropped href removes the
  attribute, leaving inert text. Verified by `mdsvex-safety.test.ts` against
  `fixtures/mdsvex-safety.svx`.

## Styling (`src/app.css`)

Plain CSS, no framework. `:root` declares `color-scheme: light dark` and a token
set (`--bg`, `--fg`, `--fg-muted`, `--fg-faint`, `--rule`, `--code`, fonts);
`@media (prefers-color-scheme: dark)` overrides the colors (`app.css:1-24`).
Layout is a centered `max-width: 42rem` column (`:74-111`). shiki output is
themed by `.body pre.shiki span { color: var(--shiki-light) }` with a dark
override (`:220-235`). Components scope their own styles (`FilterBar`, the
diagram frame/node) and rely on the same tokens.

## Tests

vitest in browser mode (playwright chromium, `vitest.config.ts:29-41`):

- `updates.test.ts` - `inlineTitleHtml`/`plainText`/`updateScript` behavior and
  invariants over `siteUpdates` (slug ids, ISO dates, lowercased tags,
  newest-first order, absolute https links, unique ids).
- `filter-expression.test.ts` - parser semantics (AND/OR/NOT, precedence,
  grouping, errors).
- `mdsvex-safety.test.ts` - mounts the fixture and asserts unsafe links are inert
  and unknown code languages fall back. See [build-deploy](../build-deploy/overview.md) for how
  these run as flake checks.
