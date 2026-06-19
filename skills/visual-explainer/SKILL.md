---
name: visual-explainer
description: "Build a visual, skimmable explainer for a piece of code, a feature, or a system: a standalone HTML page or a SvelteKit/playbook entry. Lead with a question, keep prose tight, and make every section carry a graphic (a diagram, an interactive scene, an annotated visual) instead of walls of text or pasted source. Use when the user wants to explain, document, teach, or show off how something works as a page or interactive demo, asks for a diagram-first or ADHD-friendly writeup, or wants to turn a module / PR / feature into a visual explainer. Prefers diagrams over inline code, sets up every visual with one line of context, renders any real data (polars frames, tables) byte-exact, and adds motion only when it carries meaning."
---

# visual-explainer

Build an explainer a busy reader skims and *gets*, without reading every word.
The target is either a self-contained HTML file (open it locally) or a page in a
SvelteKit site such as the ix `playbook` (`src/routes/<slug>/+page.svx`). The
medium changes; the rules below do not.

This skill is about presentation. For a deep top-down, heavily-cited code
explainer in the ix playbook, see that repo's `playbook-page` skill; the two
compose (use this one's visual rules inside that one's zoom structure).

## The rules (non-negotiable)

1. **Lead with a question.** Open with the question the reader actually has
   ("How does a finished agent close its own terminal window?"), then answer it
   in one or two sentences. High-level first, details later. No preamble.

2. **Every section carries a graphic.** Literally every one. A diagram, an
   interactive scene, an annotated table, a before/after, a labelled flow. A
   prose-only or code-only section is a failure state: if a point has no visual,
   either find the visual or cut the point.

3. **Set up every visual with one line first.** Never drop a graphic with zero
   context. One sentence of scenario before it ("Picture three terminals open,
   one of them yours.") so the reader knows what they are looking at before they
   look. A graphic that appears cold reads as noise.

4. **Prefer diagrams over code.** Do not paste big source snippets or long
   inline code. A flow the reader must understand becomes a diagram, not a
   listing. Reference the real source with a single link, not a wall of it.
   Name functions inline when needed; show a 3-line snippet only when the call
   shape itself is the point.

5. **Be concise.** Short paragraphs, one idea each. Cut the second sentence that
   restates the first. If you wrote four sentences, three of them go.

6. **Flow with segues.** Each section ends with the line that sets up the next
   ("That handles any window you can point at. The one you can't is your own.").
   The page reads as one arc, problem to resolution, not a list of headings.

7. **Render real data exactly.** When you show a data structure (a polars frame,
   a table, a tree), reproduce it byte-for-byte from the real thing, aligned.
   Generate it from the actual library and paste the exact output; never hand-type
   box-drawing characters (`│ ┆ ┬ ╪ ┴`), they drift and misalign.

8. **Motion only when it means something.** No autoplaying or looping animation.
   Interactivity is welcome when it teaches: hover a token to light the thing it
   refers to, hover a control to preview its effect (a close that shrinks the
   window). Trigger on hover, keep it still otherwise.

9. **Proper, colored icons.** Use real brand marks, not monochrome glyphs where a
   colored one exists. For language logos use the multicolor `logos` iconify set
   (`~icons/logos/python` is the real two-tone snake, not the flat simple-icons
   glyph). For a product with no logos entry, use its actual favicon or GitHub
   org avatar. Put a topic icon inline with the title, not in a separate brand row.

10. **House style.** No em dashes anywhere (use a colon, comma, or two sentences).
    No decorative emoji unless asked. Lead with the concrete fact.

## How to build it

**Pick the medium.** A one-off the user wants to glance at: a single self-contained
HTML file, then open it. Something that lives in a site: a route/component in that
app, matching its conventions (in the ix playbook, a `+page.svx` plus small Svelte
components under `src/lib/components/`).

**Theme to the host.** Read the site's CSS variables (background, surface, border,
text tiers, code-bg, mono font) and style components with them so the page works
in light and dark automatically. For standalone HTML, support
`@media (prefers-color-scheme: dark)`.

**Build the arc, one visual per beat:**
- A *scene* the reader recognizes (the windows, the cluster, the pipeline), set up
  by one sentence, optionally interactive (hover to link parts).
- A *flow diagram* for the mechanism (mermaid `FlowDiagram` in the playbook, or a
  small CSS/SVG diagram in standalone HTML). Show the happy path and the edge /
  failure branch in the same diagram: the failure mode is usually the point.
- An *annotated visual* for edge cases and validation (a table of inputs to
  verdicts with check / cross marks, a 2-column before/after, a pass-matrix), not
  a paragraph describing them.

**Diagrams: one relationship each.** A handful of nodes, not the whole graph.
Edges are relationships; do not stuff a sentence into a node label.

## Verify before done

- Open it. For HTML, open the file. For a site route, run the dev server
  (`bun run dev` / `pnpm dev` per the project) and load the page; confirm it
  renders with no compile, mdsvex, or import errors. Every component used is
  imported.
- Walk the checklist: every section has a graphic; every graphic has a one-line
  setup; no big pasted source; any data is exact and aligned; no em dashes; icons
  are colored; nothing animates on its own.
- Work in a worktree, keep the main checkout clean, and open a PR when done.
