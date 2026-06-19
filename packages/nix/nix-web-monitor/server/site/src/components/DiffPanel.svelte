<script lang="ts">
  import PanelHeader from '$lib/PanelHeader.svelte';

  type Props = {
    diff: string | null;
  };

  const { diff }: Props = $props();
</script>

<!-- Self-hides until the post-switch `nvd diff` lands, so a plain build never
     shows an empty panel. nvd output is short and pre-formatted, so a monospace
     block is enough; no parsing. -->
{#if diff !== null}
  <section class="panel diff-panel">
    <PanelHeader title="changes" />
    <div class="diff-body">
      <pre class="diff-text">{diff}</pre>
    </div>
  </section>
{/if}
