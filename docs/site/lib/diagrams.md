# lib / diagrams

`site/src/lib/diagrams/` is the interactive flowchart system embedded inside
`.svx` entries. It is built on `@xyflow/svelte` (the Svelte port of React Flow,
`site/package.json:37`). Three layers: a reusable frame
(`DiagramFrame.svelte`), one custom node type (`BoxNode.svelte`), and thin
data-only diagram wrappers (`*Diagram.svelte`). All styling uses the same
`app.css` theme tokens, so diagrams follow light/dark automatically.

## `DiagramFrame.svelte`

The reusable wrapper around `<SvelteFlow>`. Props (`:15-20`): `nodes: Node[]`,
`edges: Edge[]`, `height = 300`, `caption?`.

- **Edge defaults.** `decoratedEdges` (`:29-35`) gives each edge a `smoothstep`
  type and an `ArrowClosed` marker unless the caller overrides them.
- **Prop -> state sync.** SvelteFlow's `bind:` needs `$state` containers but the
  prop stays the source of truth, so `$effect.pre` copies props into
  `inlineNodes/inlineEdges` and `modalNodes/modalEdges` (`:40-49`). `$state.raw`
  avoids deep proxying of the graph arrays.
- **Custom node type.** `nodeTypes = { box: BoxNode }` (`:54`); the structural
  cast is needed because xyflow's `NodeTypes` index signature is wider than the
  concrete `BoxNode` type (comment `:51-53`).
- **Mount gate.** `<SvelteFlow>` needs the DOM (ResizeObserver,
  getBoundingClientRect), so it renders only after `onMount` sets `mounted`
  (`:56-61,150`). This keeps it out of the static prerender; the prerendered HTML
  has an empty, `aria-hidden` placeholder until hydration.
- **Inline view (`:150-191`).** Non-interactive: `fitView`, dragging/zoom/pan all
  disabled, attribution hidden, dotted background. A single "Expand" button opens
  the modal.
- **Modal view (`:195-259`).** A `role="dialog" aria-modal="true"` overlay with a
  backdrop button, a fully interactive `<SvelteFlow>` (drag, zoom, pan,
  `<Controls>`), and a close button. It implements a manual focus trap and Escape
  handling (`onModalKeydown` `:87-125`, `modalFocusableElements` `:127-134`),
  focuses the close button on open, restores focus to the trigger on close
  (`:81-85`), and freezes body scroll while open (`$effect` `:136-145`).
- **Theming.** Scoped `:global(...)` rules restyle the xyflow edges, arrowheads,
  edge labels, controls, and background to the `--fg-*`/`--rule`/`--bg`/`--code`
  tokens (`:397-453`); `.dashed` edges get a dash array.

## `BoxNode.svelte`

The one custom node renderer (`type: 'box'`). Data shape `BoxData` (`:4-9`):
optional `kicker`, required `label`, optional `sublabel`, and `kind`
(`'default' | 'proc' | 'artifact' | 'agent' | 'gate'`). Each `kind` is a distinct
border/background style (`:46-69`): `proc` filled, `artifact` dashed, `agent`
double border, `gate` a skewed parallelogram (with counter-skewed text). It
declares eight xyflow `Handle`s (target+source on all four sides, `:22-29`) so
edges can attach to any side via `sourceHandle`/`targetHandle`; the handle dots
are hidden but connection points stay live (`:104-112`). `label` may contain
`<code>`.

## Diagram wrappers (`*Diagram.svelte`)

Each is a data-only module: it defines `nodes`/`edges` arrays and renders one
`<DiagramFrame {nodes} {edges} height caption />`. No logic beyond layout data.

| file | embedded by | depicts |
| --- | --- | --- |
| `IxMcpDiagram.svelte` | `updates/ix-mcp-python.svx` | agent -> `ix-mcp` stdio -> python worker + session globals + recorded `lines.jsonl` |
| `RunRecorderDiagram.svelte` | `updates/run-recorder.svx` | `nix run .#run` PTY recorder capturing a command to a log |
| `AiReviewGateDiagram.svelte` | `updates/ai-review-gate.svx` | a GitHub PR event flowing through the AI review gate |

To add a diagram: create a `*Diagram.svelte` with `nodes`/`edges` (use `BoxNode`
`kind`s for visual roles) and embed it from an entry's `<script>` block (see
`updates/ix-mcp-python.svx:11-17`). It will render statically as a placeholder and
become interactive after hydration.
