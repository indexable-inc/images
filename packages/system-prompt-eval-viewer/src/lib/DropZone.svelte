<script lang="ts">
  let { active }: { active: boolean } = $props();
</script>

{#if active}
  <div class="drop">drop an eval JSON to view it</div>
{/if}

<style>
  .drop { position: fixed; inset: 14px; display: grid; place-items: center; font-size: 18px; font-weight: 500;
    color: var(--accent); background: color-mix(in oklab, var(--bg) 88%, transparent);
    border: 2px dashed var(--accent); border-radius: 12px; z-index: 50; }
</style>
