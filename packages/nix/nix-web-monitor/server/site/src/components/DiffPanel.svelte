<script lang="ts">
  type Props = {
    diff: string | null;
  };

  const { diff }: Props = $props();
</script>

<!-- The pane system hides this pane's tab until the post-switch `nvd diff`
     lands (see App.svelte), so a plain build never shows an empty panel. nvd
     output is short and pre-formatted, so a monospace block is enough; no
     parsing. -->
{#if diff !== null}
  <section class="panel diff-panel">
    <div class="diff-body">
      <pre class="diff-text">{diff}</pre>
    </div>
  </section>
{/if}
