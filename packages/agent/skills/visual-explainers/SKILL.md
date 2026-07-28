---
name: visual-explainers
description: "Build visual explanations: live boards in the test-ide fleet dashboard (default) or single-file HTML explorables (fallback), plus system diagrams and data charts. Use whenever asked to visualize something, explain a system or mechanism visually, build an explainer, diagram a pipeline or protocol, or chart data. Encodes the house philosophy: static-first (the lesson is visible on load with zero clicks), boards reuse the dashboard's component catalog and theme tokens, interaction only for navigation or minute-plus disclosures, one accent color, no decorative animation."
---

## Visual explainers

Make rich, simple, beautiful visualizations. The reader should walk away
with a deep understanding of the system after about thirty seconds of
looking, explained in plain words. Everything below serves that.

## Where they live

Default target is the fleet dashboard repo (test-ide) at `~/Projects/test`,
its standard location on every machine (home-relative, so the same recipe
works for any user). Write `canvas/boards/<your-session-id>/main.svelte`;
the running app renders it live on this run's detail page and hot-swaps
every save in.

Reuse before building. The board catalog (`$lib/components/board`) already
has `CodeFile` (editor-style file pane), `DiffView` (IntelliJ-style
side-by-side diff with center gutter bands; it computes the diff itself),
`FileIcon` (vscode-icons), `Terminal` (starship-style shell replay), and the
status primitives. Style only with the app's theme tokens (`bg-primary`,
`border-border`, `text-muted-foreground`, ...); raw palette classes read as
foreign in the shell. That repo's CLAUDE.md governs the design language.
Promote any piece another board could reuse into the catalog instead of
copying it.

Fall back to a single self-contained HTML file (inline CSS and JS, vanilla,
opens from file:// with the network off) when the dashboard repo is
unavailable or the user explicitly asks for an HTML file. In the fallback,
put multi-line code or command blocks in `pre`: newlines inside a styled
`div` collapse silently into one run-on paragraph, and the page looks fine
in the editor while shipping broken.

## Static-first: the lesson is visible on load

The full point must land with zero clicks: the diff, the mechanism, the
real numbers, all on screen at once, with attention doing the layering --
expected things faded, the surprise at full contrast. If the reader has to
press a button to see the change, the page has hidden its own lesson, and
they will not press it.

Interaction earns its place in exactly two roles:

- navigation between real artifacts (selecting a file in an explorer);
- disclosing depth that would take over a minute to read inline (a drill,
  a [why?] expander).

Never for the core claim, never as ceremony. Every moving pixel must satisfy
one of two tests: the reader caused it, or it encodes causality in the
system being explained. Delete everything else: entrance transitions, pulse
loops, parallax, hover shimmer, animated gradients.

A counterexample deepens understanding: show the world without the
mechanism as a short faded contrast block, not a second interactive mode.

## Deep but simple

Plain words, short sentences. Use real payloads, real file contents, real
commit hashes and line numbers from the actual system under discussion,
never lorem ipsum or `foo`. Show changed code as highlighted code -- diff
gutters, added-line tint -- not as abstract cards or entity lists. The test
of success is prediction, not recall: after the page, the reader can say
what the system would do in a case the page never showed.

## Visual system

- Dark-friendly. One accent color, plus at most one or two semantic colors
  (error red, success green, VCS blue for modified) used only for their
  meaning.
- Generous whitespace. Monospace for code and data tokens.
- No chart junk, no gradients as decoration, no drop shadows doing nothing.
- No edge bars: the colored stripe down the left border of a card, callout,
  quote block, or stat tile. That chrome is the signature of a generated
  dashboard and readers discount the page on sight. Separate and emphasize
  blocks with whitespace and type, never a bar.

## Data charts

When the task is an actual data chart rather than a system explainer:

- Honest axes: bar charts start at zero; never truncate an axis to
  manufacture drama; label units.
- Direct labeling over legends: put the series name at the end of its line.
- Accessible contrast for every mark and label, in dark and light.
- Interaction still earns its place: a tooltip that shows the exact value, a
  toggle that isolates one series. Nothing autoplays.

## Time

- Render timestamps in the reader's local timezone, and check what that is
  before writing any time into copy (`date +%Z` on the host, or better:
  keep the machine ISO-8601 instant in the data and let the page format it
  with `Intl.DateTimeFormat`, which is right for every reader). Hardcoded
  `13:45Z` in prose makes the reader do arithmetic; that is the page's job.
- Prefer human forms everywhere: "4h ago", "38m elapsed", "eta 5:10-5:25
  pm". Machine timestamps belong in the data layer, human time in the
  rendered layer.
- Label the zone whenever the page states an absolute clock time, since the
  artifact may be read later or from somewhere else.

## Worked example: the target shape

"How sparse flake locks work": an explorer column where the three modified
files render VCS-blue with an M badge, a DiffView already open on
flake.lock showing the exact 18 added lines at their real line numbers with
gutter bands mapping the splice across, a Terminal replaying the three real
commands with the key output line in green, and one faded paragraph of
contrast: what the same bump cost before the mechanism (a failed build, a
72-line relock). Zero required clicks; picking a different file is the only
interaction, and everything it reveals is another real artifact.

## Before shipping

- The lesson lands with zero clicks; nothing core hides behind interaction.
- Remove any motion the reader did not cause and the system does not
  explain.
- Read every annotation: is anything visible before the reader needs it?
- Replace any placeholder data that survived with values from the real
  system.
- Boards: `npm run check` in the dashboard repo passes, and
  `GET /canvas/errors` is clean after the save. HTML fallback: the page
  opens from `file://` with the network disabled.
