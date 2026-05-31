<script lang="ts">
  import { untrack } from 'svelte';
  import { store, SCOPE_SEP } from '$lib/stream.svelte';
  import { loadPositions, savePositions, autoPlace } from '$lib/positions';
  import type { Term, Point } from '$lib/types';
  import TermCard from './TermCard.svelte';

  let boardEl: HTMLElement | undefined = $state();
  // View transform: translate (tx,ty) then scale. Card positions live in board
  // space; the canvas div carries the transform.
  let tx = $state(0);
  let ty = $state(0);
  let scale = $state(1);
  let panning = $state(false);
  let grabbedKey = $state<string | null>(null);

  const positions = $state<Record<string, Point>>(loadPositions());

  const items = $derived(
    Object.keys(store.terminals)
      .sort()
      .map((key) => {
        const sep = key.indexOf(SCOPE_SEP);
        const scope = sep === -1 ? '' : key.slice(0, sep);
        return { key, term: { ...store.terminals[key], key, scope } as Term };
      }),
  );

  // Give any newly-seen terminal a starting slot. untrack so writing positions
  // does not re-trigger this effect; it only depends on the terminal set.
  $effect(() => {
    const keys = Object.keys(store.terminals);
    untrack(() => {
      let changed = false;
      for (const k of keys) {
        if (!positions[k]) {
          positions[k] = autoPlace(Object.keys(positions).length);
          changed = true;
        }
      }
      if (changed) savePositions(positions);
    });
  });

  // Manual non-passive wheel listener so preventDefault works (Svelte attaches
  // wheel as passive). Two-finger trackpad swipe pans; pinch (ctrlKey) zooms
  // about the cursor.
  $effect(() => {
    const el = boardEl;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      if (e.ctrlKey) {
        zoomAbout(e.clientX, e.clientY, Math.exp(-e.deltaY * 0.0015));
      } else {
        tx -= e.deltaX;
        ty -= e.deltaY;
      }
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  });

  // Scale by `factor` while keeping the board point under (clientX,clientY)
  // fixed on screen.
  function zoomAbout(clientX: number, clientY: number, factor: number): void {
    const el = boardEl;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const mx = clientX - rect.left;
    const my = clientY - rect.top;
    const ns = Math.min(3, Math.max(0.25, scale * factor));
    const bx = (mx - tx) / scale;
    const by = (my - ty) / scale;
    tx = mx - bx * ns;
    ty = my - by * ns;
    scale = ns;
  }

  function onPointerDown(e: PointerEvent): void {
    const target = e.target as Element;
    const node = target.closest('.node') as HTMLElement | null;
    if (node && target.closest('[data-drag-handle]')) {
      startCardDrag(e, node.dataset.key as string);
      return;
    }
    // Clicking a card body selects text; only the empty board pans.
    if (node) return;
    startPan(e);
  }

  // Run a pointer gesture: install move/end listeners on window and tear them
  // all down on either pointerup or pointercancel. pointercancel (a browser
  // gesture takeover, touch interruption, or focus loss) never fires a matching
  // pointerup, so cleaning up only on pointerup would leak listeners and leave
  // the board stuck mid-gesture.
  function runGesture(onMove: (ev: PointerEvent) => void, onEnd: () => void): void {
    const end = () => {
      onEnd();
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', end);
      window.removeEventListener('pointercancel', end);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', end);
    window.addEventListener('pointercancel', end);
  }

  function startPan(e: PointerEvent): void {
    panning = true;
    const sx = e.clientX;
    const sy = e.clientY;
    const otx = tx;
    const oty = ty;
    runGesture(
      (ev) => {
        tx = otx + (ev.clientX - sx);
        ty = oty + (ev.clientY - sy);
      },
      () => {
        panning = false;
      },
    );
  }

  function startCardDrag(e: PointerEvent, key: string): void {
    e.preventDefault();
    grabbedKey = key;
    const start = positions[key] ?? autoPlace(0);
    const sx = e.clientX;
    const sy = e.clientY;
    runGesture(
      (ev) => {
        // Convert screen delta to board space by dividing out the zoom.
        positions[key] = {
          x: start.x + (ev.clientX - sx) / scale,
          y: start.y + (ev.clientY - sy) / scale,
        };
      },
      () => {
        grabbedKey = null;
        savePositions(positions);
      },
    );
  }

  // Zoom about the board's own center (used by the HUD buttons), accounting for
  // the board sitting below the header.
  function zoomCentered(factor: number): void {
    const el = boardEl;
    if (!el) return;
    const r = el.getBoundingClientRect();
    zoomAbout(r.left + r.width / 2, r.top + r.height / 2, factor);
  }

  function resetView(): void {
    tx = 0;
    ty = 0;
    scale = 1;
  }

  function tidy(): void {
    items.forEach((it, i) => {
      positions[it.key] = autoPlace(i);
    });
    savePositions(positions);
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="board"
  class:panning
  role="application"
  aria-label="terminal board"
  bind:this={boardEl}
  onpointerdown={onPointerDown}
  style="background-position: {tx}px {ty}px; background-size: {24 * scale}px {24 * scale}px;"
>
  <div class="canvas" style="transform: translate({tx}px, {ty}px) scale({scale});">
    {#each items as it (it.key)}
      <div
        class="node"
        data-key={it.key}
        style="left: {positions[it.key]?.x ?? 0}px; top: {positions[it.key]?.y ?? 0}px;"
      >
        <TermCard term={it.term} grabbed={grabbedKey === it.key} />
      </div>
    {/each}
  </div>

  {#if items.length === 0}
    <div class="empty">
      {store.live ? 'no terminals yet' : 'connecting…'}
      <div class="hint">spawn a <code>tui.Tui(...)</code>; it shows up here automatically</div>
    </div>
  {/if}

  <div class="hud">
    <button onclick={() => zoomCentered(1 / 1.2)} aria-label="zoom out">−</button>
    <span class="zoom">{Math.round(scale * 100)}%</span>
    <button onclick={() => zoomCentered(1.2)} aria-label="zoom in">+</button>
    <button onclick={resetView}>reset</button>
    <button onclick={tidy}>tidy</button>
  </div>
</div>
