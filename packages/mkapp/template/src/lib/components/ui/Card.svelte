<script lang="ts">
  import type { Snippet } from 'svelte';

  type Props = {
    title?: string;
    children: Snippet;
  };

  let { title, children }: Props = $props();
</script>

<section class="card">
  {#if title}
    <h2>{title}</h2>
  {/if}
  <div class="body">
    {@render children()}
  </div>
</section>

<style>
  .card {
    background: var(--panel);
    border: 1px solid var(--edge);
    border-radius: var(--radius);
    padding: 16px 18px;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 14px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  /* Muted foreground for body copy, the shadcn card rhythm. */
  .body {
    color: var(--ink-dim);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
</style>
