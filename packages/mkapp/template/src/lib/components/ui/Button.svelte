<script lang="ts">
  import type { Snippet } from 'svelte';

  type Props = {
    variant?: 'default' | 'outline' | 'ghost';
    disabled?: boolean;
    onclick?: (event: MouseEvent) => void;
    children: Snippet;
  };

  let { variant = 'default', disabled = false, onclick, children }: Props = $props();
</script>

<button class="btn {variant}" {disabled} {onclick}>
  {@render children()}
</button>

<style>
  /* The shadcn button look, in plain scoped CSS on the app tokens. */
  .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    height: 32px;
    padding: 0 14px;
    border-radius: calc(var(--radius) - 2px);
    border: 1px solid transparent;
    font: 500 13px/1 var(--ui);
    letter-spacing: -0.01em;
    cursor: pointer;
    transition: opacity 120ms ease, background 120ms ease;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .default {
    background: var(--accent);
    color: light-dark(#ffffff, #08080a);
  }
  .default:hover:not(:disabled) {
    opacity: 0.9;
  }
  .outline {
    background: var(--panel);
    border-color: var(--edge-strong);
    color: var(--ink);
  }
  .outline:hover:not(:disabled) {
    background: var(--elev);
  }
  .ghost {
    background: transparent;
    color: var(--ink);
  }
  .ghost:hover:not(:disabled) {
    background: var(--accent-soft);
  }
</style>
