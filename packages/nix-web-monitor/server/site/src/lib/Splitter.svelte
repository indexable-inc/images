<script lang="ts">
  /// A draggable separator between two panes. Layout, drag math, and
  /// persistence belong to the parent; this component owns the visible track,
  /// the grip indicator, hover/focus affordance, and ARIA wiring.
  ///
  /// `orientation` matches the ARIA convention: `"vertical"` is a vertical
  /// line that resizes columns; `"horizontal"` resizes rows.

  type Orientation = 'vertical' | 'horizontal';

  type Props = {
    orientation: Orientation;
    label: string;
    valueNow: number;
    onstart: (event: PointerEvent) => void;
    onkeydown: (event: KeyboardEvent) => void;
  };

  const { orientation, label, valueNow, onstart, onkeydown }: Props = $props();
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="splitter splitter-{orientation === 'vertical' ? 'h' : 'v'}"
  role="separator"
  tabindex="0"
  aria-orientation={orientation}
  aria-label={label}
  aria-valuenow={valueNow}
  onpointerdown={onstart}
  {onkeydown}
></div>
