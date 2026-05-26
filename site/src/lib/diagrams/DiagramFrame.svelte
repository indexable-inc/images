<script lang="ts">
  import { onMount } from 'svelte';
  import {
    SvelteFlow,
    Background,
    BackgroundVariant,
    MarkerType,
    type Node,
    type Edge
  } from '@xyflow/svelte';
  import '@xyflow/svelte/dist/style.css';
  import BoxNode from './BoxNode.svelte';

  type Props = {
    nodes: Node[];
    edges: Edge[];
    height?: number;
    caption?: string;
  };

  const {
    nodes: initialNodes,
    edges: initialEdges,
    height = 300,
    caption
  }: Props = $props();

  const decoratedEdges = $derived(
    initialEdges.map((edge) => ({
      type: edge.type ?? 'smoothstep',
      markerEnd: edge.markerEnd ?? { type: MarkerType.ArrowClosed, width: 14, height: 14 },
      ...edge
    }))
  );

  // SvelteFlow's `bind:` wants a $state container, but the source of truth
  // stays the prop. Sync prop updates into the local state via $effect.pre
  // so we never read the prop inside the $state initializer.
  let nodes = $state.raw<Node[]>([]);
  let edges = $state.raw<Edge[]>([]);
  $effect.pre(() => {
    nodes = initialNodes;
    edges = decoratedEdges;
  });

  // The xyflow `NodeTypes` index signature wants `Component<Node<...>>`; our
  // BoxNode is a `Component<NodeProps<Node<BoxData,'box'>>>`. The runtime
  // shape matches, but the structural type check needs a cast.
  const nodeTypes = { box: BoxNode } as unknown as Record<string, typeof BoxNode>;

  // SvelteFlow needs the DOM (ResizeObserver, getBoundingClientRect). Defer
  // until after mount so the static prerender doesn't try to render it.
  let mounted = $state(false);
  onMount(() => {
    mounted = true;
  });
</script>

<figure class="diagram-figure">
  <div class="diagram" style="height: {height}px" aria-hidden={mounted ? undefined : 'true'}>
    {#if mounted}
      <SvelteFlow
        bind:nodes
        bind:edges
        {nodeTypes}
        fitView
        fitViewOptions={{ padding: 0.18 }}
        minZoom={0.5}
        maxZoom={1.5}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable={false}
        zoomOnScroll={false}
        zoomOnPinch={false}
        zoomOnDoubleClick={false}
        panOnDrag={false}
        panOnScroll={false}
        preventScrolling={false}
        proOptions={{ hideAttribution: false }}
      >
        <Background variant={BackgroundVariant.Dots} gap={16} size={1} />
      </SvelteFlow>
    {/if}
  </div>
  {#if caption}<figcaption>{caption}</figcaption>{/if}
</figure>

<style>
  .diagram-figure {
    margin: 1.5rem 0;
  }

  .diagram {
    border: 1px solid var(--rule);
    border-radius: 6px;
    overflow: hidden;
    background: var(--bg);
  }

  figcaption {
    margin-top: 0.55rem;
    font-family: var(--font-mono);
    font-size: 0.75rem;
    color: var(--fg-faint);
    text-align: center;
  }

  .diagram :global(.svelte-flow) {
    background: transparent;
  }

  .diagram :global(.svelte-flow__background) {
    color: var(--fg-faint);
    opacity: 0.5;
  }

  .diagram :global(.svelte-flow__edge-path) {
    stroke: var(--fg-muted);
    stroke-width: 1.4;
  }

  .diagram :global(.svelte-flow__edge.dashed .svelte-flow__edge-path) {
    stroke-dasharray: 4 3;
  }

  .diagram :global(.svelte-flow__arrowhead) {
    fill: var(--fg-muted);
  }

  .diagram :global(.svelte-flow__edge-text) {
    font-family: var(--font-mono);
    font-size: 10.5px;
    fill: var(--fg-muted);
  }

  .diagram :global(.svelte-flow__edge-textbg) {
    fill: var(--bg);
  }

  .diagram :global(.svelte-flow__attribution) {
    background: transparent;
    color: var(--fg-faint);
    font-size: 9.5px;
    padding: 2px 4px;
  }

  .diagram :global(.svelte-flow__attribution a) {
    color: var(--fg-faint);
    border: 0;
  }
</style>
