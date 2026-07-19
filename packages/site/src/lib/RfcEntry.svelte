<script lang="ts">
  import { onMount } from 'svelte';
  import { formatDay, relativeDay } from './format-posted-at';
  import { inlineTitleHtml } from './updates';
  import RfcStatusBadge from './RfcStatusBadge.svelte';
  import { rfcDimensions } from './rfcs';
  import ScoreMeter from './ScoreMeter.svelte';
  import type { Rfc } from './rfcs';

  const { rfc }: { rfc: Rfc } = $props();

  const Body = $derived(rfc.component);
  const titleHtml = $derived(inlineTitleHtml(rfc.title));
  const descriptionHtml = $derived(rfc.description ? inlineTitleHtml(rfc.description) : undefined);
  const authors = $derived(rfc.authors.split(/[,\s]+/).filter((name) => name.length > 0));

  // Relative dates need a clock; take it after mount so the prerendered
  // HTML is deterministic (same pattern as the homepage timestamps).
  let now = $state<number | undefined>(undefined);
  onMount(() => {
    now = Date.now();
  });

  function day(iso: string): string {
    if (now === undefined || Number.isNaN(Date.parse(iso))) return formatDay(iso);
    return `${formatDay(iso)} · ${relativeDay(iso, now)}`;
  }
</script>

<article id={rfc.id}>
  <p class="eyebrow">RFC {rfc.number}</p>
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
    <dt>Authors</dt>
    <dd class="authors">
      {#each authors as author (author)}
        <a class="author" href={`https://github.com/${author}`}>
          <img class="avatar" src={`https://github.com/${author}.png?size=48`} alt="" />
          {author}
        </a>
      {/each}
    </dd>
    <dt>Created</dt><dd><time datetime={rfc.created}>{day(rfc.created)}</time></dd>
    <dt>Updated</dt><dd><time datetime={rfc.updated}>{day(rfc.updated)}</time></dd>
    <dt>Tracking issue</dt><dd>{rfc.trackingIssue ?? '—'}</dd>
    <dt>Supersedes</dt><dd>{rfc.supersedes ?? '—'}</dd>
    <dt>Superseded by</dt><dd>{rfc.supersededBy ?? '—'}</dd>
  </dl>
  <div class="body">
    <Body />
  </div>
</article>

<style>
  .eyebrow {
    font-family: var(--font-mono);
    font-size: 0.75rem;
    letter-spacing: 0.06em;
    color: var(--fg-faint);
    text-transform: uppercase;
    margin: 0 0 -0.5rem;
  }

  .authors {
    display: flex;
    align-items: center;
    gap: 0.9rem;
    flex-wrap: wrap;
  }

  .author {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
  }

  .avatar {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: 1px solid var(--rule);
  }

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
