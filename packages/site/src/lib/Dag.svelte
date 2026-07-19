<script lang="ts">
  // Shared plan-structure diagram: nodes on a coarse grid, edges as arrows.
  // Every plan that needs a DAG uses this one renderer so they all read the
  // same way. Coordinates are grid cells, not pixels.
  export type DagNode = {
    id: string;
    label: string;
    // Optional second line.
    sub?: string;
    // Grid position: column and row, 0-based.
    col: number;
    row: number;
    // done = landed, active = in progress, todo (default) = not started.
    state?: 'done' | 'active' | 'todo';
  };

  const {
    nodes,
    edges
  }: {
    nodes: DagNode[];
    edges: [string, string][];
  } = $props();

  // Cell-grid geometry: node height and row pitch are whole multiples of
  // the 21px --cell-h so the diagram sits on the page's character grid.
  const CW = 210;
  const RH = 105;
  const NW = 180;
  const NH = 63;
  const PAD = 21;

  function cx(node: DagNode): number {
    return PAD + node.col * CW + NW / 2;
  }
  function cy(node: DagNode): number {
    return PAD + node.row * RH + NH / 2;
  }

  const byId = $derived(new Map(nodes.map((n) => [n.id, n])));
  const width = $derived(PAD * 2 + (Math.max(...nodes.map((n) => n.col)) + 1) * CW - (CW - NW));
  const height = $derived(PAD * 2 + (Math.max(...nodes.map((n) => n.row)) + 1) * RH - (RH - NH));

  function edgePath(from: string, to: string): string {
    const a = byId.get(from);
    const b = byId.get(to);
    if (!a || !b) throw new Error(`dag edge references unknown node: ${from} -> ${to}`);
    const x1 = cx(a) + NW / 2;
    const y1 = cy(a);
    const x2 = cx(b) - NW / 2;
    const y2 = cy(b);
    const mid = (x1 + x2) / 2;
    const s = (n: number): string => n.toFixed(1);
    return `M ${s(x1)} ${s(y1)} C ${s(mid)} ${s(y1)}, ${s(mid)} ${s(y2)}, ${s(x2)} ${s(y2)}`;
  }
</script>

<figure class="dag">
  <!-- Fixed 1:1 pixels, not viewBox scaling: labels keep the site's one
       14px cell everywhere; narrow screens scroll like a wide terminal pane. -->
  <svg {width} {height} viewBox="0 0 {width} {height}" role="img" aria-label="Plan structure diagram">
    <defs>
      <marker id="dag-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto">
        <path d="M 0 0 L 8 4 L 0 8 z" fill="var(--fg-faint)" />
      </marker>
    </defs>
    {#each edges as edge (edge[0] + edge[1])}
      <path class="edge" d={edgePath(edge[0], edge[1])} marker-end="url(#dag-arrow)" />
    {/each}
    {#each nodes as node (node.id)}
      <g class={`node ${node.state ?? 'todo'}`}>
        <rect x={cx(node) - NW / 2} y={cy(node) - NH / 2} width={NW} height={NH} rx="0" />
        <text class="label" x={cx(node)} y={node.sub ? cy(node) - 5 : cy(node) + 5} text-anchor="middle">
          {node.label}
        </text>
        {#if node.sub}
          <text class="sub" x={cx(node)} y={cy(node) + 16} text-anchor="middle">{node.sub}</text>
        {/if}
      </g>
    {/each}
  </svg>
</figure>

<style>
  .dag {
    margin: var(--cell-h) 0;
    overflow-x: auto;
  }

  svg {
    display: block;
  }

  .edge {
    fill: none;
    stroke: var(--fg-faint);
    stroke-width: 1;
  }

  .node rect {
    fill: var(--code);
    stroke: var(--rule);
  }

  .node.done rect {
    fill: color-mix(in srgb, var(--status-load-bearing) 10%, transparent);
    stroke: var(--status-load-bearing);
  }

  .node.active rect {
    fill: color-mix(in srgb, var(--status-input-wanted) 10%, transparent);
    stroke: var(--status-input-wanted);
  }

  .label {
    font-family: var(--font-mono);
    font-size: 14px;
    fill: var(--fg);
  }

  .sub {
    font-family: var(--font-mono);
    font-size: 14px;
    fill: var(--fg-muted);
  }
</style>
