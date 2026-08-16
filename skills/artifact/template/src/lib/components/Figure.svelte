<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    caption,
    children
  }: {
    caption?: string;
    children?: Snippet;
  } = $props();
</script>

<figure>
  {@render children?.()}
  {#if caption}
    <figcaption>{caption}</figcaption>
  {/if}
</figure>
