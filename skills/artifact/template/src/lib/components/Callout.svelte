<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    kind = 'note',
    children
  }: {
    kind?: 'note' | 'tip' | 'warn';
    children?: Snippet;
  } = $props();

  const glyphs = { note: 'i', tip: '*', warn: '!' } as const;
</script>

<!-- Tinted block, no left edge bar: emphasis comes from tone and type. -->
<aside class="callout {kind}">
  <span class="glyph">{glyphs[kind]}</span>
  <div class="body">
    {@render children?.()}
  </div>
</aside>

<style>
  .callout {
    display: flex;
    gap: 0.75rem;
    align-items: baseline;
    border-radius: 10px;
    /* A tint alone is ~1.1:1 against the dark page, so the block reads
       as a stray paragraph; the border is what makes it a block. Mixed
       from the foreground rather than --border, which is itself only
       1.25:1 on dark and would barely show. */
    border: 1px solid color-mix(in srgb, var(--fg-muted) 28%, transparent);
    padding: 0.85rem 1.1rem;
    margin: 1.25rem 0;
    font-size: 0.95rem;
  }

  .callout.note {
    background: var(--bg-raised);
  }

  .callout.tip {
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 26%, transparent);
  }

  .callout.warn {
    background: var(--warn-bg);
    border-color: color-mix(in srgb, var(--warn-fg) 34%, transparent);
  }

  .glyph {
    font-family: var(--font-mono);
    font-weight: 700;
    color: var(--fg-muted);
  }

  .callout.tip .glyph {
    color: var(--accent);
  }

  /* Kind must survive without color, but the glyph is the only carrier
     here, so at least give warn its own hue rather than muted grey. */
  .callout.warn .glyph {
    color: var(--warn-fg);
  }

  .body :global(p:first-child) {
    margin-top: 0;
  }

  .body :global(p:last-child) {
    margin-bottom: 0;
  }
</style>
