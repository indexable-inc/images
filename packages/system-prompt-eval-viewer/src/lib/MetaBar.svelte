<script lang="ts">
  import Chip from './ui/Chip.svelte';
  let { entries }: { entries: [string, unknown][] } = $props();
</script>

<div class="meta">
  {#each entries as [k, v]}
    <Chip label={k} value={v} />
  {/each}
</div>

<style>
  .meta { display: flex; flex-wrap: wrap; gap: 6px; margin: 10px 0 16px; }
</style>
