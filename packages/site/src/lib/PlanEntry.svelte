<script lang="ts">
  import { onMount } from 'svelte';
  import { formatDay, relativeDay } from './format-posted-at';
  import { inlineTitleHtml } from './updates';
  import StatusBadge from './StatusBadge.svelte';
  import { planDimensions } from './plans';
  import ScoreMeter from './ScoreMeter.svelte';
  import { scoreOf } from './scores';
  import type { Plan } from './plans';

  const { plan }: { plan: Plan } = $props();

  const Body = $derived(plan.component);
  const titleHtml = $derived(inlineTitleHtml(plan.title));
  const descriptionHtml = $derived(plan.description ? inlineTitleHtml(plan.description) : undefined);
  const authors = $derived(plan.authors.split(/[,\s]+/).filter((name) => name.length > 0));

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

<article id={plan.id}>
  <p class="eyebrow">Plan {plan.number}</p>
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
    <dt>Status</dt>
    <dd>
      <StatusBadge status={plan.status} />
      {#if plan.rfc}
        <StatusBadge status="RFC" />
      {/if}
    </dd>
    {#each planDimensions as dim (dim.key)}
      {@const score = scoreOf(plan.id, plan.scores, dim.key)}
      <dt>{dim.label}</dt>
      <dd class="score">
        <ScoreMeter value={score.value} />
        <span class="value">{score.value}</span>
        <span class="why">{score.why}</span>
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
    <dt>Created</dt><dd><time datetime={plan.created}>{day(plan.created)}</time></dd>
    <dt>Updated</dt><dd><time datetime={plan.updated}>{day(plan.updated)}</time></dd>
    <dt>Tracking issue</dt><dd>{plan.trackingIssue ?? '-'}</dd>
    <dt>Supersedes</dt><dd>{plan.supersedes ?? '-'}</dd>
    <dt>Superseded by</dt><dd>{plan.supersededBy ?? '-'}</dd>
  </dl>
  <div class="body">
    <Body />
  </div>
</article>

<style>
  .eyebrow {
    letter-spacing: 0.08em;
    color: var(--fg-faint);
    text-transform: uppercase;
    margin: 0;
  }

  .authors {
    display: flex;
    align-items: center;
    gap: 0 2ch;
    flex-wrap: wrap;
  }

  .author {
    display: inline-flex;
    align-items: center;
    gap: 1ch;
    text-decoration: none;
  }

  .author:hover {
    background: var(--fg);
    color: var(--bg);
  }

  /* One cell square, minus the hairline so the row stays one cell tall. */
  .avatar {
    width: calc(var(--cell-h) - 2px);
    height: calc(var(--cell-h) - 2px);
    border-radius: var(--radius);
    border: 1px solid var(--rule);
  }

  .score {
    display: flex;
    align-items: baseline;
    gap: 2ch;
  }

  .score .value {
    font-variant-numeric: tabular-nums;
    color: var(--fg-muted);
    min-width: 2ch;
  }

  .score .why {
    color: var(--fg-muted);
  }
</style>
