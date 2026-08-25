---
name: artifact
description: "Make an artifact: a beautiful, Notion-like single-page Svelte explainer, or a keyboard-navigable slide deck, scaffolded from this skill's template (bun dev, mdsvex + shiki, prebuilt components, versioned content with a diff selector; the slides variant adds a second HTML entry with dot navigation and a changed-since-last-view indicator). Use when asked to make an artifact, visualize something, explain a system or mechanism visually, build an explainer, diagram a pipeline or protocol, chart data, make slides, build a slide deck, or put together a presentation. Replaces visual-explainers; encodes the house philosophy: static-first, one accent color, real data, no decorative animation."
---

## Artifacts

An artifact is a small standalone page that teaches one thing well. The
reader should walk away with a deep understanding after about thirty
seconds of looking, explained in plain words. Sleek is a requirement:
Notion-calm typography, generous whitespace, dark and light both first
class.

## Scaffold

The `template/` directory sits next to this SKILL.md. Scaffold by
copying it (no script: committed shell is fenced, #3823):

    cp -R <this-skill-dir>/template <dest-dir>
    rm -rf <dest-dir>/node_modules <dest-dir>/dist
    cd <dest-dir> && bun install && bun dev

`<dest-dir>` must not already exist (refuse, do not merge). The rm -rf
drops any build state that leaked into the template. Scaffold into a
scratch or session working directory, not into a repo you happen to be
sitting in. `bun dev` serves Vite with hot reload;
every save renders immediately.

Home-manager trap (measured 2026-08-24): when this skill dir is HM-managed,
so `cp -R template` copies SYMLINKS into /nix/store and the copy is
read-only. Scaffold with `cp -RL` (dereference) AND `chmod -R u+w
<dest-dir>` -- store 444 modes survive the dereference. Verify:
`find <dest-dir> -type l | wc -l` must be 0. Also: after an external
edit, Vite's transform cache can serve a stale module on the bare URL;
curl with a cache-buster (`?t=N`). Worse (disproven 2026-08-24): an
externally-written .svx may stay stale even on fresh page loads and after
`touch` -- the glob-importer's compiled module caches it. Verify the
RENDERED page; if stale, restart `bun dev`.

Batteries already wired in the template:

- **svx pages** (mdsvex): markdown prose with Svelte components inline.
- **Syntax highlighting**: fenced code blocks render through shiki with
  dual themes; unknown languages fall back to plain text instead of
  crashing the build.
- **Components** in `src/lib/components/`: `Callout` (note/tip/warn,
  tinted block, no edge bar), `Figure` (captioned diagram or SVG),
  `DiffView`, `VersionPicker`, `ThemeToggle`.
- **Theme tokens** in `src/app.css` (`--bg`, `--fg`, `--accent`,
  `--add-*`, `--del-*`, ...). Style with tokens, never raw hex in
  components, so both themes stay correct. Inline SVG follows the theme
  by using `style="fill: var(--accent)"` (CSS vars do not work in bare
  SVG presentation attributes).

Extend the template in place -- add components, add data files -- it is
a working copy, not a vendored dependency.

## Versions

Content lives in `src/versions/v0.svx, v1.svx, ...` -- one file per
version, immutable once superseded. To revise, copy the highest
`v<n>.svx` to `v<n+1>.svx` and edit that; never rewrite an old version.
The loader picks new files up automatically; the header shows version
tabs and a `diff` mode that renders a folded line diff between any two
versions. Give each version frontmatter:

    ---
    title: Debounce vs throttle
    note: adds the timeline figure
    ---

The `note` says what changed and appears in the diff header. Identity
is the filename ordinal plus the file's own bytes; the diff is computed
from raw sources at build time. There is deliberately no CRDT here:
versions are written sequentially by one author, so append-only files
diff cleanly and stay reviewable in the repo.

## Slides

For a keyboard-navigable deck instead of a single-page explainer, the
template ships a second, independent Vite entry -- `slides.html` +
`src/slides/main.ts` + `src/slides/Deck.svelte` -- served by the same
`bun dev`, zero extra config (Vite's dev server serves any `.html` file
at the project root on request). Open `/slides.html` alongside `/`.
Edit `Deck.svelte` in place: each slide is a `{#snippet}`, listed in the
array rendered by `{@render [s0, s1, ...][i]()}`; add a slide by adding
a snippet and an array entry, and bump `N`. The `box` snippet plus a
`.flow` row is the reusable "labeled box, chain with an arrow" primitive
for pipeline/architecture slides; style with the same `--fg`, `--accent`,
`--bg-raised` tokens as the explainer, not raw hex.

Navigation: `ArrowRight` / `Space` advances, `ArrowLeft` / `Shift+Space`
goes back, `Home`/`End` jump to the first/last slide, and the dot row at
the bottom is clickable.

Changed-since-last-view indicator: a single `REV` constant plus a
`changes: Record<slideIndex, {rev, note}>` map drive it, checked against
a per-slide `seen` map persisted to `localStorage`. A dot gets an accent
ring when its slide's change `rev` is newer than what the viewer has
seen; the note pill above the dots shows the current slide's note only
while its `rev` still equals the live `REV` (so an old note doesn't
linger once revs move on). **Maintenance contract: every content edit
bumps `REV` and adds a `{rev, note}` entry for the slide that changed** --
skipping this silently disables the indicator for that edit.

## Recommend visuals

Prose alone is usually the weakest form. When building an artifact,
actively propose the visual that carries the argument:

- **Structure or flow** (pipeline, protocol, architecture): an inline
  SVG diagram with real component names, in reading order.
- **Change** (before/after, regression, refactor): a diff, with real
  line numbers, not two prose paragraphs.
- **Comparison** (tradeoffs, alternatives): a table with the deciding
  row first.
- **Quantity** (latency, counts, sizes): a chart with honest axes --
  bars start at zero, units labeled, series named at the line's end,
  never a truncated axis for drama.
- **Timing or ordering** (races, debounce, scheduling): an event
  timeline.

Use real payloads, real file contents, real numbers from the system
under discussion; never lorem ipsum or `foo`. The test of success is
prediction: after the page, the reader can say what the system would do
in a case the page never showed.

## Static-first

The full point lands with zero clicks: the mechanism, the numbers, the
surprise, all on screen on load, attention doing the layering (expected
things faded, the surprise at full contrast). Interaction earns its
place in exactly two roles: navigation between real things (the version
picker is this), and disclosing depth that would take over a minute to
read inline. Never for the core claim. Every moving pixel must be
caused by the reader or encode causality in the system explained;
delete entrance transitions, pulse loops, hover shimmer, animated
gradients.

## Visual system

- One accent color, plus at most one or two semantic colors (error red,
  success green) used only for their meaning.
- Monospace for code and data tokens; no chart junk; no drop shadows
  doing nothing.
- No edge bars (the colored stripe down a callout's left border): that
  chrome reads as generated dashboard and the reader discounts the page
  on sight. Separate blocks with whitespace and tone.
- Human time in the rendered layer ("4h ago", "eta 5:10-5:25 pm"),
  ISO-8601 instants in the data layer, formatted with
  `Intl.DateTimeFormat` so every reader sees their own zone.

## Fallback: single HTML file

When bun or a dev server is unavailable, or the user asks for a file
they can open directly, fall back to one self-contained HTML file:
inline CSS and JS, vanilla, opens from `file://` with the network off.
Put multi-line code in `pre` -- newlines inside a styled `div` collapse
silently and the page ships broken while looking fine in the editor.

## Before shipping

- The lesson lands with zero clicks; nothing core hides behind
  interaction.
- Both themes checked; code blocks highlighted, not plain.
- No placeholder data survived; annotations appear when needed, not
  before.
- A new revision went into a new `v<n>.svx` and the diff against its
  predecessor reads as the changelog.
