<script lang="ts">
  import { resolve } from '$app/paths';
  import { inlineTitleHtml } from '$lib/updates';
  import { rfcs } from '$lib/rfcs';
  import RfcStatusBadge from '$lib/RfcStatusBadge.svelte';
  import ScoreChart from '$lib/ScoreChart.svelte';
  import { rfcDimensions } from '$lib/rfcs';

  function statusSlug(status: string): string {
    return status.toLowerCase().replace(/\s+/g, '-');
  }

  const chartItems = rfcs.map((rfc) => ({
    id: rfc.id,
    label: rfc.number,
    title: rfc.title,
    detail: rfc.status,
    colorVar: `var(--status-${statusSlug(rfc.status)})`,
    href: resolve('/rfcs/[id]', { id: rfc.id }),
    scores: rfc.scores
  }));

  const chartStatuses = [...new Set(rfcs.map((rfc) => rfc.status))];
</script>

<svelte:head>
  <title>RFCs · index</title>
  <meta name="description" content="Design documents for non-trivial changes to this repo." />
</svelte:head>

<h1>RFCs</h1>
<p class="subtitle">Design documents for non-trivial changes to this repo.</p>

<p>
  RFCs capture <em>why</em> a decision was made and what alternatives were considered, which <code
  >git log</code> does not.
</p>

<h2>Map</h2>
<p>
  Every RFC scores itself 1-10 in frontmatter along six axes: ambition (incremental to moonshot),
  impact, effort, risk, maturity (rough to battle-tested), and leverage (one-off to flywheel).
  Every score carries a one-clause why, so a number you disagree with is a PR away from a better
  one. Pick any two axes; click a dot to read the RFC.
</p>
<div class="chart-legend">
  {#each chartStatuses as status (status)}
    <RfcStatusBadge {status} />
  {/each}
</div>
<ScoreChart items={chartItems} dimensions={rfcDimensions} initialX="ambition" initialY="impact" />

<h2>Index</h2>
<ul class="rfc-index">
  {#each rfcs as rfc (rfc.id)}
    <li>
      <span class="num">{rfc.number}</span>
      <a href={resolve('/rfcs/[id]', { id: rfc.id })}>
        <!-- eslint-disable-next-line svelte/no-at-html-tags -->
        {@html inlineTitleHtml(rfc.title)}
      </a>
      <RfcStatusBadge status={rfc.status} />
    </li>
  {/each}
</ul>

<h2>When to write one</h2>
<p>
  Open an RFC for changes that touch shared abstractions: the fleet API, module conventions, the
  trust model, networking primitives, lint rules, or anything that asks contributors to do
  something noticeably differently. Bug fixes, refactors that do not change a public surface, and
  one-off additions do not need an RFC; a normal PR is enough.
</p>
<p>If you are unsure, open an RFC. The cost is low.</p>

<h2>Process</h2>
<ol>
  <li>
    Copy <a href={resolve('/rfcs/[id]', { id: '0000-template' })}><code>0000-template.svx</code></a>
    to <code>packages/site/src/lib/rfcs/NNNN-short-slug.svx</code>, using the next free number.
  </li>
  <li>
    Fill in the frontmatter (status starts at <code>Draft</code>, or <code>Sketch</code> if it is
    mostly a vibe; score the four 1-10 axes honestly) and the body.
  </li>
  <li>Open a PR titled <code>RFC NNNN: &lt;short title&gt;</code>.</li>
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
  depends on. Move an RFC up the ladder with a follow-up PR that edits the frontmatter.
</p>
<ul>
  <li>
    <RfcStatusBadge status="Sketch" />: an idea written down so it is not lost. Little human
    review yet; may be largely machine-drafted.
  </li>
  <li>
    <RfcStatusBadge status="Draft" />: a coherent proposal the author stands behind. Open to
    feedback. Default for a freshly merged RFC.
  </li>
  <li>
    <RfcStatusBadge status="Input wanted" />: the author has thought hard about it and now wants
    more human eyes before going further.
  </li>
  <li>
    <RfcStatusBadge status="Last call" />: final thoughts wanted. Accepted absent objections
    within a stated window.
  </li>
  <li>
    <RfcStatusBadge status="Accepted" />: the design is the plan of record. Implementation may
    not be started.
  </li>
  <li>
    <RfcStatusBadge status="Load-bearing" />: the proposal landed and the repo now depends on it.
    Link the tracking issue and the PRs.
  </li>
  <li>
    <RfcStatusBadge status="Rejected" />: a follow-up PR set this status. Keep the file so the
    reasoning is preserved.
  </li>
  <li><RfcStatusBadge status="Withdrawn" />: the author no longer pursues it. Same retention rule.</li>
  <li>
    <RfcStatusBadge status="Superseded" />: pointed at a newer RFC via <code>supersededBy</code> in
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
  Once an RFC is <code>Accepted</code>, file a GitHub issue tagged <code>rfc-implementation</code>
  that links the RFC. The issue tracks the work; the RFC remains the design source of truth. When
  the work lands, a follow-up PR sets the RFC status to <code>Load-bearing</code> and links the
  issue and PRs from the frontmatter.
</p>

<h2>Why Svelte/mdsvex</h2>
<p>
  RFCs are <code>.svx</code> files (markdown plus frontmatter) rendered by the same SvelteKit site
  as the rest of <code>index</code>'s public pages, instead of self-contained HTML. One shared
  stylesheet and one shiki-highlighted code path serve every RFC and every other page, rather than
  each file carrying its own copy of the same ~70 lines of CSS. The source is still plain text a PR
  review reads as prose: markdown diffs cleanly, and headings/lists/code blocks are lighter than
  the HTML they replace.
</p>

<style>
  .rfc-index {
    list-style: none;
    padding-left: 0;
  }

  .rfc-index li {
    display: flex;
    align-items: baseline;
    gap: 0.65rem;
    margin: 0.45rem 0;
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
