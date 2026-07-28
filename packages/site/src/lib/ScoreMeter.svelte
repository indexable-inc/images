<script lang="ts">
  const { value, max = 10 }: { value: number; max?: number } = $props();

  // Block-element characters render the meter as text on the cell grid,
  // the way a TUI draws a gauge: filled full blocks, light-shade rest.
  const filled = $derived('█'.repeat(value));
  const rest = $derived('░'.repeat(Math.max(max - value, 0)));
</script>

<span class="meter" role="img" aria-label={`${String(value)} of ${String(max)}`}
  ><span class="filled" aria-hidden="true">{filled}</span><span
    class="rest"
    aria-hidden="true">{rest}</span
  ></span
>

<style>
  .meter {
    white-space: pre;
    letter-spacing: 0;
  }

  .filled {
    color: var(--fg-muted);
  }

  .rest {
    color: color-mix(in srgb, var(--fg-faint) 45%, transparent);
  }
</style>
