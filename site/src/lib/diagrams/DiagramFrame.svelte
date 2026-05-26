<script lang="ts">
  import { onMount } from 'svelte';
  import {
    SvelteFlow,
    Background,
    BackgroundVariant,
    Controls,
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
  let inlineNodes = $state.raw<Node[]>([]);
  let inlineEdges = $state.raw<Edge[]>([]);
  let modalNodes = $state.raw<Node[]>([]);
  let modalEdges = $state.raw<Edge[]>([]);
  $effect.pre(() => {
    inlineNodes = initialNodes;
    inlineEdges = decoratedEdges;
    modalNodes = initialNodes;
    modalEdges = decoratedEdges;
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

  let expanded = $state(false);
  let expandButton: HTMLButtonElement | undefined = $state();
  let closeButton: HTMLButtonElement | undefined = $state();

  function openExpanded(): void {
    expanded = true;
  }

  function closeExpanded(): void {
    expanded = false;
    // Return focus to the trigger for keyboard users.
    queueMicrotask(() => expandButton?.focus());
  }

  function onModalKeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
      event.preventDefault();
      closeExpanded();
    }
  }

  $effect(() => {
    if (!expanded) return;
    // Prevent body scroll while the overlay is open.
    const previous = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    queueMicrotask(() => closeButton?.focus());
    return () => {
      document.body.style.overflow = previous;
    };
  });
</script>

<figure class="diagram-figure">
  <div class="diagram" style="height: {height}px" aria-hidden={mounted ? undefined : 'true'}>
    {#if mounted}
      <SvelteFlow
        bind:nodes={inlineNodes}
        bind:edges={inlineEdges}
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
        proOptions={{ hideAttribution: true }}
      >
        <Background variant={BackgroundVariant.Dots} gap={16} size={1} />
      </SvelteFlow>
      <button
        type="button"
        class="expand-button"
        aria-label="Expand diagram"
        title="Expand"
        onclick={openExpanded}
        bind:this={expandButton}
      >
        <svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <path
            d="M2 6V2h4M14 6V2h-4M2 10v4h4M14 10v4h-4"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
            stroke-linecap="round"
          />
        </svg>
      </button>
    {/if}
  </div>
  {#if caption}<figcaption>{caption}</figcaption>{/if}
</figure>

{#if mounted && expanded}
  <div
    class="overlay"
    role="dialog"
    aria-modal="true"
    aria-label={caption ?? 'Expanded diagram'}
    onkeydown={onModalKeydown}
    tabindex="-1"
  >
    <button
      type="button"
      class="backdrop"
      aria-label="Close expanded diagram"
      onclick={closeExpanded}
    ></button>
    <div class="modal" role="presentation">
      <header class="modal-header">
        <span class="modal-caption">{caption ?? ''}</span>
        <button
          type="button"
          class="close-button"
          aria-label="Close expanded diagram"
          onclick={closeExpanded}
          bind:this={closeButton}
        >
          <svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
            <path
              d="M4 4l8 8M12 4l-8 8"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
              stroke-linecap="round"
            />
          </svg>
        </button>
      </header>
      <div class="modal-body">
        <SvelteFlow
          bind:nodes={modalNodes}
          bind:edges={modalEdges}
          {nodeTypes}
          fitView
          fitViewOptions={{ padding: 0.1 }}
          minZoom={0.25}
          maxZoom={3}
          nodesDraggable
          nodesConnectable={false}
          elementsSelectable={false}
          zoomOnScroll
          zoomOnPinch
          zoomOnDoubleClick
          panOnDrag
          panOnScroll={false}
          preventScrolling
          proOptions={{ hideAttribution: true }}
        >
          <Background variant={BackgroundVariant.Dots} gap={20} size={1} />
          <Controls showLock={false} />
        </SvelteFlow>
      </div>
    </div>
  </div>
{/if}

<style>
  .diagram-figure {
    margin: 1.5rem 0;
  }

  .diagram {
    position: relative;
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

  .expand-button {
    position: absolute;
    top: 0.5rem;
    right: 0.5rem;
    z-index: 5;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.75rem;
    height: 1.75rem;
    padding: 0;
    border: 1px solid var(--rule);
    border-radius: 4px;
    background: var(--bg);
    color: var(--fg-muted);
    cursor: pointer;
    transition:
      color 0.15s ease,
      border-color 0.15s ease;
  }

  .expand-button:hover,
  .expand-button:focus-visible {
    color: var(--fg);
    border-color: var(--fg-muted);
    outline: none;
  }

  .expand-button svg {
    width: 0.95rem;
    height: 0.95rem;
  }

  .overlay {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2rem;
  }

  .backdrop {
    position: absolute;
    inset: 0;
    border: 0;
    padding: 0;
    margin: 0;
    background: color-mix(in srgb, var(--bg) 78%, transparent);
    backdrop-filter: blur(6px);
    cursor: zoom-out;
  }

  .modal {
    position: relative;
    z-index: 1;
    display: flex;
    flex-direction: column;
    width: min(100%, 1100px);
    height: min(100%, 720px);
    background: var(--bg);
    border: 1px solid var(--rule);
    border-radius: 8px;
    box-shadow: 0 14px 38px rgb(0 0 0 / 0.18);
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.55rem 0.75rem 0.55rem 1rem;
    border-bottom: 1px solid var(--rule);
  }

  .modal-caption {
    font-family: var(--font-mono);
    font-size: 0.75rem;
    color: var(--fg-muted);
  }

  .close-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.85rem;
    height: 1.85rem;
    padding: 0;
    border: 1px solid var(--rule);
    border-radius: 4px;
    background: var(--bg);
    color: var(--fg-muted);
    cursor: pointer;
  }

  .close-button:hover,
  .close-button:focus-visible {
    color: var(--fg);
    border-color: var(--fg-muted);
    outline: none;
  }

  .close-button svg {
    width: 1rem;
    height: 1rem;
  }

  .modal-body {
    position: relative;
    flex: 1;
    min-height: 0;
  }

  .diagram :global(.svelte-flow),
  .modal-body :global(.svelte-flow) {
    background: transparent;
  }

  .diagram :global(.svelte-flow__background),
  .modal-body :global(.svelte-flow__background) {
    color: var(--fg-faint);
    opacity: 0.5;
  }

  .diagram :global(.svelte-flow__edge-path),
  .modal-body :global(.svelte-flow__edge-path) {
    stroke: var(--fg-muted);
    stroke-width: 1.4;
  }

  .diagram :global(.svelte-flow__edge.dashed .svelte-flow__edge-path),
  .modal-body :global(.svelte-flow__edge.dashed .svelte-flow__edge-path) {
    stroke-dasharray: 4 3;
  }

  .diagram :global(.svelte-flow__arrowhead),
  .modal-body :global(.svelte-flow__arrowhead) {
    fill: var(--fg-muted);
  }

  .diagram :global(.svelte-flow__edge-text),
  .modal-body :global(.svelte-flow__edge-text) {
    font-family: var(--font-mono);
    font-size: 10.5px;
    fill: var(--fg-muted);
  }

  .diagram :global(.svelte-flow__edge-textbg),
  .modal-body :global(.svelte-flow__edge-textbg) {
    fill: var(--bg);
  }

  .modal-body :global(.svelte-flow__controls) {
    border: 1px solid var(--rule);
    border-radius: 4px;
    overflow: hidden;
    box-shadow: none;
  }

  .modal-body :global(.svelte-flow__controls-button) {
    background: var(--bg);
    color: var(--fg-muted);
    border-bottom: 1px solid var(--rule);
    fill: currentColor;
  }

  .modal-body :global(.svelte-flow__controls-button:hover) {
    background: var(--code);
    color: var(--fg);
  }
</style>
