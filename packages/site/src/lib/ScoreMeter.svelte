<script lang="ts">
  const { value, max = 10 }: { value: number; max?: number } = $props();
</script>

<span class="meter" role="img" aria-label={`${String(value)} of ${String(max)}`}>
  {#each Array.from({ length: max }) as unused, i (i)}
    <span class="cell" class:filled={i < value}></span>
  {/each}
</span>

<style>
  .meter {
    display: inline-flex;
    gap: 2px;
    vertical-align: middle;
  }

  .cell {
    width: 9px;
    height: 9px;
    border-radius: 2px;
    background: var(--code);
  }

  .cell.filled {
    background: var(--fg-muted);
  }
</style>
