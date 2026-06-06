<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    ariaLabel: string;
    className?: string;
    role?: string;
    width?: string;
    children?: Snippet;
    onpointerenter?: (event: PointerEvent) => void;
    onpointerleave?: (event: PointerEvent) => void;
  }

  let {
    ariaLabel,
    className = '',
    role = 'menu',
    width = '124px',
    children,
    onpointerenter,
    onpointerleave
  }: Props = $props();
</script>

<div
  class="picker-menu {className}"
  style:width={width}
  {role}
  aria-label={ariaLabel}
  onpointerenter={onpointerenter}
  onpointerleave={onpointerleave}
>
  {@render children?.()}
</div>

<style>
  .picker-menu {
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-elev);
    box-shadow: 0 16px 36px rgba(0, 0, 0, 0.32);
  }
</style>
