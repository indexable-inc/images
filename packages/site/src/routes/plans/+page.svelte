<script lang="ts">
  import { resolve } from '$app/paths';
  import { inlineTitleHtml } from '$lib/updates';
  import { plans } from '$lib/plans';
  import StatusBadge from '$lib/StatusBadge.svelte';
  import ScoreChart from '$lib/ScoreChart.svelte';
  import { planDimensions } from '$lib/plans';

  function statusSlug(status: string): string {
    return status.toLowerCase().replace(/\s+/g, '-');
  }

  const chartItems = plans.map((plan) => ({
    id: plan.id,
    label: plan.number,
    title: plan.title,
    detail: plan.status,
    colorVar: `var(--status-${statusSlug(plan.status)})`,
    href: resolve('/plans/[id]', { id: plan.id }),
    scores: plan.scores
  }));

  const chartStatuses = [...new Set(plans.map((plan) => plan.status))];
</script>

<svelte:head>
  <title>Plans · index</title>
  <meta name="description" content="Design documents for non-trivial changes to this repo." />
</svelte:head>

<h1>Plans</h1>
<p class="subtitle">Design documents for non-trivial changes to this repo.</p>

<p>
  Plans capture <em>why</em> a decision was made and what alternatives were considered, which <code
  >git log</code> does not.
</p>

<h2>Map</h2>
<p>
  Every Plan scores itself 1-10 in frontmatter along the axes defined below, and every score
  carries a one-clause why, so a number you disagree with is a PR away from a better one. Pick
  any two axes; click a dot to read the Plan.
</p>
<div class="chart-legend">
  {#each chartStatuses as status (status)}
    <StatusBadge {status} />
  {/each}
</div>
<ScoreChart items={chartItems} dimensions={planDimensions} initialX="ambition" initialY="impact" />

<h2>Index</h2>
<ul class="plan-index">
  {#each plans as plan (plan.id)}
    <li>
      <a href={resolve('/plans/[id]', { id: plan.id })}>
        <!-- eslint-disable-next-line svelte/no-at-html-tags -->
        {@html inlineTitleHtml(plan.title)}
      </a>
      <span class="num">{plan.number}</span>
      <StatusBadge status={plan.status} />
      {#if plan.rfc}
        <StatusBadge status="RFC" />
      {/if}
    </li>
  {/each}
</ul>

<h2>Dimensions</h2>
<dl class="dims">
  {#each planDimensions as dim (dim.key)}
    <dt>{dim.label} <span class="range">{dim.low} → {dim.high}</span></dt>
    <dd>{dim.meaning}</dd>
  {/each}
</dl>

<h2>When to write one</h2>
<p>
  Open an Plan for changes that touch shared abstractions: the fleet API, module conventions, the
  trust model, networking primitives, lint rules, or anything that asks contributors to do
  something noticeably differently. Bug fixes, refactors that do not change a public surface, and
  one-off additions do not need an Plan; a normal PR is enough.
</p>
<p>If you are unsure, open an Plan. The cost is low.</p>

<h2>Process</h2>
<ol>
  <li>
    Copy <a href={resolve('/plans/[id]', { id: '0000-template' })}><code>0000-template.svx</code></a>
    to <code>packages/site/src/lib/plans/NNNN-short-slug.svx</code>, using the next free number.
  </li>
  <li>
    Fill in the frontmatter (status starts at <code>Draft</code>, or <code>Sketch</code> if it is
    mostly a vibe; score the four 1-10 axes honestly) and the body.
  </li>
  <li>Open a PR titled <code>Plan NNNN: &lt;short title&gt;</code>.</li>
  <li>PR review is the discussion. Line comments are the unit of feedback.</li>
  <li>
    Merge when the proposal is coherent enough to read, even if it is not "finished". Subsequent
    edits land as follow-up PRs against the same file. The status field in the frontmatter tracks
    lifecycle; PR state is just how edits get in.
  </li>
</ol>

<h2>Status values</h2>
<p>
  Statuses form a ladder of weight, from a vibe someone wrote down to a design the whole repo
  depends on. Move an Plan up the ladder with a follow-up PR that edits the frontmatter.
</p>
<ul>
  <li>
    <StatusBadge status="Sketch" />: an idea written down so it is not lost. Little human
    review yet; may be largely machine-drafted.
  </li>
  <li>
    <StatusBadge status="Draft" />: a coherent proposal the author stands behind. Open to
    feedback. Default for a freshly merged Plan.
  </li>
  <li>
    <StatusBadge status="Input wanted" />: the author has thought hard about it and now wants
    more human eyes before going further.
  </li>
  <li>
    <StatusBadge status="Last call" />: final thoughts wanted. Accepted absent objections
    within a stated window.
  </li>
  <li>
    <StatusBadge status="Accepted" />: the design is the plan of record. Implementation may
    not be started.
  </li>
  <li>
    <StatusBadge status="Load-bearing" />: the proposal landed and the repo now depends on it.
    Link the tracking issue and the PRs.
  </li>
  <li>
    <StatusBadge status="Rejected" />: a follow-up PR set this status. Keep the file so the
    reasoning is preserved.
  </li>
  <li><StatusBadge status="Withdrawn" />: the author no longer pursues it. Same retention rule.</li>
  <li>
    <StatusBadge status="Superseded" />: pointed at a newer Plan via <code>supersededBy</code> in
    frontmatter.
  </li>
</ul>

<h2>Numbering</h2>
<p>
  Numbers are zero-padded to four digits and never reused. If two PRs race for the same number,
  the later one renames before merge.
</p>

<h2>Implementation tracking</h2>
<p>
  Once an Plan is <code>Accepted</code>, file a GitHub issue tagged <code>plan-implementation</code>
  that links the Plan. The issue tracks the work; the Plan remains the design source of truth. When
  the work lands, a follow-up PR sets the Plan status to <code>Load-bearing</code> and links the
  issue and PRs from the frontmatter.
</p>

<h2>Why Svelte/mdsvex</h2>
<p>
  Plans are <code>.svx</code> files (markdown plus frontmatter) rendered by the same SvelteKit site
  as the rest of <code>index</code>'s public pages, instead of self-contained HTML. One shared
  stylesheet and one shiki-highlighted code path serve every Plan and every other page, rather than
  each file carrying its own copy of the same ~70 lines of CSS. The source is still plain text a PR
  review reads as prose: markdown diffs cleanly, and headings/lists/code blocks are lighter than
  the HTML they replace.
</p>

<style>
  .plan-index {
    list-style: none;
    padding-left: 0;
  }

  .plan-index li {
    display: flex;
    align-items: baseline;
    gap: 0.65rem;
    margin: 0.45rem 0;
  }

  .dims dt {
    font-weight: 600;
    margin-top: 0.7rem;
  }

  .dims .range {
    font-family: var(--font-mono);
    font-size: 0.75em;
    font-weight: 400;
    color: var(--fg-faint);
    margin-left: 0.5rem;
  }

  .dims dd {
    margin: 0.15rem 0 0;
    color: var(--fg-muted);
    max-width: 65ch;
  }

  .chart-legend {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin-bottom: 0.25rem;
  }

  .num {
    font-family: var(--font-mono);
    font-size: 0.8em;
    color: var(--fg-faint);
    flex-shrink: 0;
  }
</style>
