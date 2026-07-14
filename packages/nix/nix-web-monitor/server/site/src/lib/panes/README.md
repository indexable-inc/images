# panes — a small, dependency-free Svelte pane system

Modular panes for arbitrary content, VS Code/ghostty style: split layouts with
draggable resizing, tab groups, pop-out floating windows, collapse/expand, and
layout persistence. Built for the nix web monitor but written against a generic
API so it can be extracted into a shared package later (each site in this repo
is a self-contained npm project built by `ix.buildSvelteSite`, so a shared
frontend package needs its own build plumbing first — this module deliberately
has **zero** imports from the rest of the app and no third-party dependencies
beyond Svelte itself).

## Usage

```svelte
<script lang="ts">
  import PaneDock from '$lib/panes/PaneDock.svelte';
  import { group, split, type DockLayout, type PaneSpec } from '$lib/panes/types';

  let dock = $state<PaneDock | null>(null);

  function defaultLayout(): DockLayout {
    return {
      root: split('column', [0.75, 0.25], [
        split('row', [0.7, 0.3], [group(['main']), group(['side-a', 'side-b'])]),
        group(['logs'])
      ]),
      floating: []
    };
  }

  function paneSpecs(main: Snippet, sideA: Snippet, ...): PaneSpec[] {
    return [
      { id: 'main', title: 'main', content: main },
      { id: 'side-a', title: 'side a', content: sideA, visible: someCondition },
      ...
    ];
  }
</script>

{#snippet main()}<MyMainView />{/snippet}
{#snippet sideA()}<OtherView />{/snippet}

<PaneDock
  bind:this={dock}
  storageKey="my-app.pane-layout"
  {defaultLayout}
  panes={paneSpecs(main, sideA, ...)}
/>
```

- **Panes are snippets.** A pane is `{ id, title, content }` plus optionally
  `controls` (rendered on the tab bar's right while the pane is active — the
  slot for filter chips, sort selects, counters) and `visible` (set false to
  hide the tab everywhere without losing its layout slot).
- **Panes stay mounted.** A group renders every visible tab's snippet and
  hides the inactive ones (and all of them while collapsed) with CSS, so
  pane-local state — filters, expanded rows, scroll positions — survives tab
  switches and collapse, like a fixed layout that never unmounts panels. The
  flip side: a hidden pane's effects keep running. Dragging a tab to another
  group (or popping it out) still remounts it.
- **Layout is a tree.** Internal nodes are `split` (row/column, fractional
  sizes); leaves are `group` (tabs sharing a region). `types.ts` has `split()`
  / `group()` constructors.
- **Everything persists.** Any arrangement change is written as JSON under
  `storageKey`; a stored layout that fails validation (or an old schema
  version) falls back to `defaultLayout()`. `dock.resetLayout()` is the reset
  affordance; `dock.reveal(id)` activates/uncollapses/raises a pane.

## Interactions

- Drag the boundary between panes to resize (keyboard: focus it, arrows;
  Shift for bigger steps).
- Drag a tab onto another group's tab bar to move it there (onto a specific
  tab to insert before it).
- `⧉` pops the active pane out into a floating window *inside the page* — not
  `window.open`, on purpose: the app is fed by one live WebSocket store and a
  real browser popup would lose it. Drag the title bar to move, the corner to
  resize, `⇲` to dock back (into the first group, since its original group may
  have been pruned when it left).
- `▾` collapses a group to its tab bar; inside a row split it collapses to a
  sideways strip.

## Theming

Styles are scoped per component and read the host's CSS custom properties,
with neutral fallbacks: `--bg`, `--panel`, `--panel-soft`, `--ink`, `--muted`,
`--faint`, `--line`, `--line-soft`, `--accent`.

## Files

- `types.ts` — data model (`PaneSpec`, `LayoutNode`, `DockLayout`) and
  constructors; plain JSON, no runtime deps.
- `layout.svelte.ts` — `DockState`: every mutation, normalization
  (no empty groups, no single-child splits), reconciliation against the
  registered pane set, and localStorage load/validate/persist.
- `context.ts` — dock-scoped context plumbing.
- `PaneDock.svelte` — root component; imperative `resetLayout()` / `reveal()`.
- `PaneSplit.svelte` — recursive split renderer + resize math.
- `PaneSplitter.svelte` — the draggable boundary (pointer capture + ARIA).
- `PaneGroup.svelte` — tab bar, tab drag-and-drop, collapse, pop-out.
- `PaneWindow.svelte` — the floating window (move/resize/dock-back).
