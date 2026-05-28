<script lang="ts">
  import type { Snippet } from 'svelte';

  type Props = {
    title: string;
    children?: Snippet;
  };

  const { title, children }: Props = $props();
</script>

<header class="panel-title">
  <span class="panel-title-text">{title}</span>
  {#if children}{@render children()}{/if}
</header>
