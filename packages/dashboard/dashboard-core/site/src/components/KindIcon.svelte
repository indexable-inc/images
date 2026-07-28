<script lang="ts">
  // A small IDE-autocomplete-style glyph per namespace kind, in place of a text
  // chip — frames read as a table, sequences as brackets, scalars as a hash, and
  // so on. Monochrome `currentColor` so the parent tints it by kind. 16px box,
  // 1.4px stroke to match the rest of the rail iconography.
  let { kind }: { kind: string } = $props();
</script>

<span class="ns-icon" data-kind={kind} aria-hidden="true">
  <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">
    {#if kind === 'frame' || kind === 'array'}
      <!-- table / grid -->
      <rect x="2.5" y="3" width="11" height="10" rx="1.2" />
      <path d="M2.5 6.5h11M6 6.5V13" />
    {:else if kind === 'mapping' || kind === 'object'}
      <!-- braces -->
      <path d="M6.2 3c-1.2 0-1.6.6-1.6 1.9 0 1.1-.2 1.8-1.1 2.1.9.3 1.1 1 1.1 2.1 0 1.3.4 1.9 1.6 1.9" />
      <path d="M9.8 3c1.2 0 1.6.6 1.6 1.9 0 1.1.2 1.8 1.1 2.1-.9.3-1.1 1-1.1 2.1 0 1.3-.4 1.9-1.6 1.9" />
    {:else if kind === 'sequence'}
      <!-- brackets -->
      <path d="M6.5 3H4.3v10h2.2M9.5 3h2.2v10H9.5" />
    {:else if kind === 'scalar'}
      <!-- hash (number) -->
      <path d="M6.2 3 5 13M11 3l-1.2 10M3.4 6.4h9.2M2.8 9.6h9.2" />
    {:else if kind === 'text'}
      <!-- text lines -->
      <path d="M3.5 4.5h9M3.5 8h9M3.5 11.5h5.5" />
    {:else if kind === 'module'}
      <!-- package / cube -->
      <path d="M8 2.5 13 5v6l-5 2.5L3 11V5zM3 5l5 2.5 5-2.5M8 7.5V13" />
    {:else if kind === 'function'}
      <!-- callable: parentheses -->
      <path d="M6 3.2c-1.6 1.8-1.6 7.8 0 9.6M10 3.2c1.6 1.8 1.6 7.8 0 9.6" />
    {:else if kind === 'class'}
      <!-- class: C -->
      <path d="M11.5 5.2A4.2 4.2 0 1 0 11.5 10.8" />
    {:else}
      <!-- fallback: a small diamond -->
      <path d="M8 3.5 12.5 8 8 12.5 3.5 8z" />
    {/if}
  </svg>
</span>
