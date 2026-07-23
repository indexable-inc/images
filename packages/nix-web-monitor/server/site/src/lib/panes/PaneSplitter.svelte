<script lang="ts">
  /// The draggable boundary between two children of a split. Owns the pointer
  /// capture and the visible track; the parent split owns the actual size math
  /// (this component only reports pixel deltas and keyboard steps).
  ///
  /// `direction` is the *split's* axis: a `row` split needs a vertical divider
  /// that resizes columns, a `column` split a horizontal one that resizes rows.

  import type { SplitDirection } from '$lib/panes/types';

  type Props = {
    direction: SplitDirection;
    label: string;
    /// Current share of the leading neighbor, as a 0-100 percentage for ARIA.
    valuePercent: number;
    /// A neighbor is collapsed; the divider renders but cannot drag.
    disabled: boolean;
    ondrag: (deltaPx: number) => void;
    ondragend: () => void;
    /// Keyboard resize: direction sign and whether Shift asked for a big step.
    onkeystep: (sign: -1 | 1, big: boolean) => void;
  };

  const { direction, label, valuePercent, disabled, ondrag, ondragend, onkeystep }: Props =
    $props();

  let dragging = $state(false);
  let last = 0;

  function coord(event: PointerEvent): number {
    return direction === 'row' ? event.clientX : event.clientY;
  }

  function onpointerdown(event: PointerEvent): void {
    if (disabled) return;
    event.preventDefault();
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    last = coord(event);
    dragging = true;
    // Pointer capture routes events here but the cursor still tracks whatever
    // is under the pointer, so pin the resize cursor for the whole drag.
    document.body.style.cursor = direction === 'row' ? 'col-resize' : 'row-resize';
    document.body.style.userSelect = 'none';
  }

  function onpointermove(event: PointerEvent): void {
    if (!dragging) return;
    const current = coord(event);
    ondrag(current - last);
    last = current;
  }

  function onpointerup(): void {
    if (!dragging) return;
    dragging = false;
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    ondragend();
  }

  function onkeydown(event: KeyboardEvent): void {
    if (disabled) return;
    const grow = direction === 'row' ? 'ArrowRight' : 'ArrowDown';
    const shrink = direction === 'row' ? 'ArrowLeft' : 'ArrowUp';
    if (event.key !== grow && event.key !== shrink) return;
    event.preventDefault();
    onkeystep(event.key === grow ? 1 : -1, event.shiftKey);
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="pane-splitter {direction}"
  class:dragging
  class:disabled
  role="separator"
  tabindex={disabled ? -1 : 0}
  aria-orientation={direction === 'row' ? 'vertical' : 'horizontal'}
  aria-label={label}
  aria-valuenow={Math.round(valuePercent)}
  {onpointerdown}
  {onpointermove}
  {onpointerup}
  onpointercancel={onpointerup}
  onlostpointercapture={onpointerup}
  {onkeydown}
></div>

<style>
  /* Visible 1px line + 5px hit area: a centered hairline keeps the layout
   * light while the whole 5px stays grabbable; grip dots appear on
   * hover/focus for affordance. */
  .pane-splitter {
    position: relative;
    background: transparent;
    touch-action: none;
  }

  .pane-splitter::before {
    content: '';
    position: absolute;
    inset: 0;
    margin: auto;
    background: var(--line, #d4d4d8);
    transition: background 100ms ease-out;
  }

  .pane-splitter::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    background: var(--faint, #9ca3af);
    border-radius: 1px;
    opacity: 0;
    transition:
      opacity 120ms ease-out,
      background 100ms ease-out;
  }

  .pane-splitter:not(.disabled):hover::before,
  .pane-splitter:focus-visible::before,
  .pane-splitter.dragging::before {
    background: var(--accent, #2563eb);
  }

  .pane-splitter:not(.disabled):hover::after,
  .pane-splitter:focus-visible::after {
    opacity: 0.75;
  }

  .pane-splitter.dragging::after {
    opacity: 0.9;
    background: var(--accent, #2563eb);
  }

  .pane-splitter.row {
    cursor: col-resize;
  }

  .pane-splitter.row::before {
    width: 1px;
    height: 100%;
  }

  .pane-splitter.row::after {
    width: 2px;
    height: 28px;
  }

  .pane-splitter.column {
    cursor: row-resize;
  }

  .pane-splitter.column::before {
    height: 1px;
    width: 100%;
  }

  .pane-splitter.column::after {
    height: 2px;
    width: 28px;
  }

  .pane-splitter.disabled {
    cursor: default;
  }

  .pane-splitter:focus-visible {
    outline: none;
  }
</style>
