<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    ariaChecked?: boolean | 'true' | 'false';
    ariaExpanded?: boolean;
    ariaHaspopup?: 'menu' | 'listbox' | 'tree' | 'grid' | 'dialog' | boolean;
    className?: string;
    role?: string;
    selected?: boolean;
    children?: Snippet;
    onclick?: (event: MouseEvent) => void;
    onfocus?: (event: FocusEvent) => void;
    onpointerenter?: (event: PointerEvent) => void;
  }

  let {
    ariaChecked,
    ariaExpanded,
    ariaHaspopup,
    className = '',
    role = 'menuitem',
    selected = false,
    children,
    onclick,
    onfocus,
    onpointerenter
  }: Props = $props();
</script>

<button
  type="button"
  class="picker-menu-item {className}"
  class:selected
  {role}
  aria-checked={ariaChecked}
  aria-expanded={ariaExpanded}
  aria-haspopup={ariaHaspopup}
  onclick={onclick}
  onfocus={onfocus}
  onpointerenter={onpointerenter}
>
  {@render children?.()}
</button>

<style>
  .picker-menu-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-height: 32px;
    min-width: 0;
    padding: 5px 8px;
    border-radius: 6px;
    color: var(--text-muted);
    font-size: 13px;
    line-height: 1.25;
    text-align: left;
  }
  .picker-menu-item:hover,
  .picker-menu-item.selected {
    background: var(--bg-pill-hi);
    color: var(--text-strong);
  }
  .picker-menu-item :global(.picker-item-label) {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .picker-menu-item :global(.picker-item-icon),
  .picker-menu-item :global(.picker-item-check),
  .picker-menu-item :global(.picker-item-chevron) {
    width: 14px;
    height: 14px;
    flex-shrink: 0;
    color: currentColor;
    opacity: 0.85;
  }
  .picker-menu-item :global(.picker-item-check),
  .picker-menu-item :global(.picker-item-chevron) {
    margin-left: auto;
  }
</style>
