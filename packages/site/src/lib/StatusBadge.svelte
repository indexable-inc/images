<script lang="ts">
  const { status }: { status: string } = $props();
  // 'Last call' -> 'last-call': keys the shared --status-* palette in
  // tokens.css (the site's one semantic-color family).
  const slug = $derived(status.toLowerCase().replace(/\s+/g, '-'));
</script>

<span class="badge {slug}" style:--c={`var(--status-${slug})`}>{status}</span>

<style>
  /* TUI chip: [ STATUS ] in the status color; brackets carry the frame so
     no border disturbs the character grid. */
  .badge {
    display: inline-block;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    font-weight: 700;
    color: var(--c);
    white-space: nowrap;
  }

  .badge::before {
    content: '[';
    font-weight: 400;
    color: color-mix(in srgb, var(--c) 55%, transparent);
  }

  .badge::after {
    content: ']';
    font-weight: 400;
    color: color-mix(in srgb, var(--c) 55%, transparent);
  }

  /* Not-going-anywhere states read hollow: regular weight, dimmed. */
  .sketch,
  .withdrawn,
  .superseded {
    font-weight: 400;
  }
</style>
