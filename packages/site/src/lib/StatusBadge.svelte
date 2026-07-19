<script lang="ts">
  const { status }: { status: string } = $props();
  // 'Last call' -> 'last-call': keys the shared --status-* palette in app.css.
  const slug = $derived(status.toLowerCase().replace(/\s+/g, '-'));
</script>

<span class="badge {slug}" style:--c={`var(--status-${slug})`}>{status}</span>

<style>
  .badge {
    display: inline-block;
    font-family: var(--font-mono);
    font-size: 0.68rem;
    line-height: 1;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    padding: 0.28em 0.6em;
    border-radius: 999px;
    color: var(--c);
    background: color-mix(in srgb, var(--c) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--c) 35%, transparent);
    white-space: nowrap;
    vertical-align: middle;
  }

  /* Not-going-anywhere states read as hollow. */
  .sketch,
  .withdrawn,
  .superseded {
    border-style: dashed;
  }
</style>
