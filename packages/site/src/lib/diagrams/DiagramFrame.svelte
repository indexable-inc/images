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
    // Inline canvas height in whole 21px cells; border-box sizing keeps the
    // frame's 1px rules inside, so it occupies exactly that many grid rows.
    heightCells?: number;
    caption?: string;
  };

  const {
    nodes: initialNodes,
    edges: initialEdges,
    heightCells = 15,
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
  let overlay: HTMLDivElement | undefined = $state();

  const focusableSelector = [
    'a[href]',
    'button:not([disabled])',
    'input:not([disabled])',
    'select:not([disabled])',
    'textarea:not([disabled])',
    '[tabindex]:not([tabindex="-1"])'
  ].join(',');

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
      return;
    }

    if (event.key !== 'Tab') {
      return;
    }

    const focusable = modalFocusableElements();
    // `.at()` includes undefined in its return type under any tsconfig, so
    // the emptiness guard type-checks for strict consumers and this app alike.
    const first = focusable.at(0);
    const last = focusable.at(-1);
    if (first === undefined || last === undefined) {
      event.preventDefault();
      overlay?.focus();
      return;
    }

    const active = document.activeElement;

    if (!(active instanceof HTMLElement) || !overlay?.contains(active)) {
      event.preventDefault();
      first.focus();
      return;
    }

    if (event.shiftKey && active === first) {
      event.preventDefault();
      last.focus();
      return;
    }

    if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  }

	  function modalFocusableElements(): HTMLElement[] {
	    if (!overlay) return [];
	    return Array.from(overlay.querySelectorAll<HTMLElement>(focusableSelector)).filter(
	      (element) =>
	        !element.classList.contains('backdrop') &&
	        (element.offsetParent !== null || element === document.activeElement)
	    );
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
  <div class="diagram" style="height: calc(var(--cell-h) * {heightCells})" aria-hidden={mounted ? undefined : 'true'}>
    {#if mounted}
      <SvelteFlow
        bind:nodes={inlineNodes}
        bind:edges={inlineEdges}
        {nodeTypes}
        fitView
        fitViewOptions={{ padding: 0.18 }}
        minZoom={0.1}
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
    bind:this={overlay}
  >
	    <button
	      type="button"
	      class="backdrop"
	      tabindex="-1"
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
    margin: var(--cell-h) 0;
  }

  .diagram {
    position: relative;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--bg);
  }

  figcaption {
    margin-top: var(--cell-h);
    color: var(--fg-faint);
    text-align: center;
  }

  /* [+] expand control, sized to the cell. */
  .expand-button {
    position: absolute;
    top: var(--cell-h);
    right: 2ch;
    z-index: 5;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--cell-h);
    height: var(--cell-h);
    padding: 0;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--fg-muted);
    cursor: pointer;
  }

  .expand-button:hover,
  .expand-button:focus-visible {
    color: var(--bg);
    background: var(--fg);
    border-color: var(--fg);
    outline: none;
  }

  .expand-button svg {
    width: 12px;
    height: 12px;
  }

  .overlay {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--cell-h) 2ch;
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
    width: min(100%, 132ch);
    height: min(100%, calc(var(--cell-h) * 34));
    background: var(--bg);
    border: 1px solid var(--fg-muted);
    border-radius: var(--radius);
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 2ch;
    padding: calc(var(--cell-h) - 1px) 2ch var(--cell-h);
    border-bottom: 1px solid var(--rule);
  }

  .modal-caption {
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .close-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--cell-h);
    height: var(--cell-h);
    padding: 0;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--fg-muted);
    cursor: pointer;
  }

  .close-button:hover,
  .close-button:focus-visible {
    color: var(--bg);
    background: var(--fg);
    border-color: var(--fg);
    outline: none;
  }

  .close-button svg {
    width: 12px;
    height: 12px;
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
    border-radius: var(--radius);
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
