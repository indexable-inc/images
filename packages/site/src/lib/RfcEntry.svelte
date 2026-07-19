<script lang="ts">
  import { inlineTitleHtml } from './updates';
  import RfcStatusBadge from './RfcStatusBadge.svelte';
  import { rfcDimensions } from './rfcs';
  import ScoreMeter from './ScoreMeter.svelte';
  import type { Rfc } from './rfcs';

  const { rfc }: { rfc: Rfc } = $props();

  const Body = $derived(rfc.component);
  const titleHtml = $derived(inlineTitleHtml(`RFC ${rfc.number}: ${rfc.title}`));
  const descriptionHtml = $derived(rfc.description ? inlineTitleHtml(rfc.description) : undefined);
</script>

<article id={rfc.id}>
  <h1>
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    {@html titleHtml}
  </h1>
  {#if descriptionHtml}
    <p class="subtitle">
      <!-- eslint-disable-next-line svelte/no-at-html-tags -->
      {@html descriptionHtml}
    </p>
  {/if}
  <dl class="frontmatter">
    <dt>Status</dt><dd><RfcStatusBadge status={rfc.status} /></dd>
    {#each rfcDimensions as dim (dim.key)}
      <dt>{dim.label}</dt>
      <dd class="score">
        <ScoreMeter value={rfc.scores[dim.key].value} />
        <span class="value">{rfc.scores[dim.key].value}</span>
        <span class="why">{rfc.scores[dim.key].why}</span>
      </dd>
    {/each}
    <dt>Authors</dt><dd>{rfc.authors}</dd>
    <dt>Created</dt><dd>{rfc.created}</dd>
    <dt>Updated</dt><dd>{rfc.updated}</dd>
    <dt>Tracking issue</dt><dd>{rfc.trackingIssue ?? '—'}</dd>
    <dt>Supersedes</dt><dd>{rfc.supersedes ?? '—'}</dd>
    <dt>Superseded by</dt><dd>{rfc.supersededBy ?? '—'}</dd>
  </dl>
  <div class="body">
    <Body />
  </div>
</article>

<style>
  .score {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  .score .value {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    font-size: 0.85em;
    color: var(--fg-muted);
    min-width: 1.4em;
  }

  .score .why {
    color: var(--fg-muted);
  }
</style>
