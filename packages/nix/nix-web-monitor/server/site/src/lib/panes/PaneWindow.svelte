<script lang="ts">
  /// A popped-out pane: a floating, draggable, resizable window layered over
  /// the dock. Deliberately *not* a browser popup -- the app runs off one live
  /// WebSocket store, so the window stays in-page and shares that state; the
  /// dock button folds it back into a tab group.

  import { untrack } from 'svelte';
  import { getDockContext } from '$lib/panes/context';
  import type { FloatingPane } from '$lib/panes/types';

  type Props = {
    floating: FloatingPane;
  };

  const { floating }: Props = $props();
  const dock = getDockContext();

  const spec = $derived(dock.spec(floating.id));

  const MIN_WIDTH = 220;
  const MIN_HEIGHT = 120;

  type DragKind = 'move' | 'resize';
  let drag: { kind: DragKind; startX: number; startY: number; baseA: number; baseB: number } | null =
    null;

  function bounds(): { width: number; height: number } {
    const root = dock.dockElement();
    if (root === null) throw new Error('pane dock root unavailable');
    return { width: root.clientWidth, height: root.clientHeight };
  }

  function clampToDock(): void {
    const box = bounds();
    const width = Math.max(MIN_WIDTH, Math.min(floating.width, box.width));
    const height = Math.max(MIN_HEIGHT, Math.min(floating.height, box.height));
    const x = Math.max(48 - width, Math.min(floating.x, box.width - 48));
    const y = Math.max(0, Math.min(floating.y, box.height - 32));
    if (
      x === floating.x &&
      y === floating.y &&
      width === floating.width &&
      height === floating.height
    ) {
      return;
    }
    dock.state.moveFloating(floating.id, x, y);
    dock.state.resizeFloating(floating.id, width, height);
    dock.state.persist();
  }

  $effect(() => {
    const root = dock.dockElement();
    if (root === null) return;
    untrack(clampToDock);
    const observer = new ResizeObserver(() => {
      untrack(clampToDock);
    });
    observer.observe(root);
    return () => {
      observer.disconnect();
    };
  });

  function start(event: PointerEvent, kind: DragKind): void {
    // A drag from an interactive control (the dock button, pane controls)
    // must keep its click semantics.
    if (kind === 'move' && event.target instanceof Element && event.target.closest('button, input, select, a') !== null) {
      return;
    }
    event.preventDefault();
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    drag =
      kind === 'move'
        ? { kind, startX: event.clientX, startY: event.clientY, baseA: floating.x, baseB: floating.y }
        : {
            kind,
            startX: event.clientX,
            startY: event.clientY,
            baseA: floating.width,
            baseB: floating.height
          };
  }

  function move(event: PointerEvent): void {
    if (drag === null) return;
    const dx = event.clientX - drag.startX;
    const dy = event.clientY - drag.startY;
    const box = bounds();
    if (drag.kind === 'move') {
      // Keep at least a grabbable sliver of the title bar inside the dock.
      const x = Math.max(48 - floating.width, Math.min(drag.baseA + dx, box.width - 48));
      const y = Math.max(0, Math.min(drag.baseB + dy, box.height - 32));
      dock.state.moveFloating(floating.id, x, y);
    } else {
      const width = Math.max(MIN_WIDTH, Math.min(drag.baseA + dx, box.width));
      const height = Math.max(MIN_HEIGHT, Math.min(drag.baseB + dy, box.height));
      dock.state.resizeFloating(floating.id, width, height);
      // The position clamp depends on the size: a pane parked at the
      // left-edge limit (x = 48 - width) that then shrinks would keep no
      // sliver inside the dock. Re-clamp against the new dimensions.
      clampToDock();
    }
  }

  function end(): void {
    if (drag === null) return;
    drag = null;
    dock.state.persist();
  }
</script>

{#if spec !== undefined && spec.visible !== false}
  <section
    class="pane-window"
    style="left: {String(floating.x)}px; top: {String(floating.y)}px; width: {String(
      floating.width
    )}px; height: {String(floating.height)}px"
    onpointerdowncapture={() => {
      dock.state.raise(floating.id);
    }}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <header
      class="pane-window-bar"
      onpointerdown={(event) => {
        start(event, 'move');
      }}
      onpointermove={move}
      onpointerup={end}
      onpointercancel={end}
    >
      <span class="pane-window-title">{spec.title}</span>
      {#if spec.controls !== undefined}
        <div class="pane-controls">{@render spec.controls()}</div>
      {/if}
      <button
        type="button"
        class="pane-action"
        title="dock back"
        aria-label="dock {spec.title} back"
        onclick={() => {
          dock.state.dock(floating.id);
        }}
      >
        ⇲
      </button>
    </header>
    <div class="pane-content">
      {@render spec.content()}
    </div>
    <div
      class="pane-window-resize"
      aria-hidden="true"
      onpointerdown={(event) => {
        start(event, 'resize');
      }}
      onpointermove={move}
      onpointerup={end}
      onpointercancel={end}
    ></div>
  </section>
{/if}

<style>
  .pane-window {
    position: absolute;
    /* Floating windows must paint over everything docked. Docked pane
     * content creates positioned boxes of its own (the build tree's sticky
     * .root-row sits at z-index 1), and a plain positioned ancestor without
     * a z-index does not contain them, so without this a docked pane's
     * sticky header would bleed through a window dragged over it. One shared
     * band well above any content z-index keeps windows on top while
     * DOM order (see `raise`) still decides the stacking between windows. */
    z-index: 10;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--panel, #ffffff);
    border: 1px solid var(--line, #d4d4d8);
    border-radius: 6px;
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.25);
    overflow: hidden;
  }

  .pane-window-bar {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0.6rem;
    border-bottom: 1px solid var(--line, #d4d4d8);
    background: var(--panel-soft, #f1f5f9);
    cursor: grab;
    user-select: none;
    touch-action: none;
  }

  .pane-window-bar:active {
    cursor: grabbing;
  }

  .pane-window-title {
    color: var(--muted, #6b7280);
    text-transform: uppercase;
    font-weight: 700;
    letter-spacing: 0.05em;
    white-space: nowrap;
  }

  .pane-controls {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
    margin-left: auto;
  }

  .pane-controls:has(+ .pane-action) {
    margin-right: 0.2rem;
  }

  .pane-window-bar > .pane-action:last-child {
    margin-left: auto;
  }

  .pane-controls + .pane-action {
    margin-left: 0;
  }

  .pane-action {
    appearance: none;
    border: 0;
    background: transparent;
    color: var(--faint, #9ca3af);
    font-family: inherit;
    cursor: pointer;
    padding: 0 0.15rem;
    line-height: 1;
  }

  .pane-action:hover {
    color: var(--ink, #111827);
  }

  .pane-content {
    flex: 1 1 auto;
    min-height: 0;
    min-width: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .pane-window-resize {
    position: absolute;
    right: 0;
    bottom: 0;
    width: 14px;
    height: 14px;
    cursor: nwse-resize;
    touch-action: none;
    background: linear-gradient(
      135deg,
      transparent 0 50%,
      var(--line, #d4d4d8) 50% 60%,
      transparent 60% 70%,
      var(--line, #d4d4d8) 70% 80%,
      transparent 80%
    );
  }
</style>
