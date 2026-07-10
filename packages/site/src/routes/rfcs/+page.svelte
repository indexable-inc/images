<script lang="ts">
  import { resolve } from '$app/paths';
  import { inlineTitleHtml } from '$lib/updates';
  import { rfcs } from '$lib/rfcs';
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

<h2>Index</h2>
<ul class="rfc-index">
  {#each rfcs as rfc (rfc.id)}
    <li>
      <a href={resolve('/rfcs/[id]', { id: rfc.id })}>
        <!-- eslint-disable-next-line svelte/no-at-html-tags -->
        {@html inlineTitleHtml(`RFC ${rfc.number}: ${rfc.title}`)}
      </a>
      <span class="status">· {rfc.status}</span>
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
  <li>Fill in the frontmatter (status starts at <code>Draft</code>) and the body.</li>
  <li>Open a PR titled <code>RFC NNNN: &lt;short title&gt;</code>.</li>
  <li>PR review is the discussion. Line comments are the unit of feedback.</li>
  <li>
    Merge when the proposal is coherent enough to read, even if it is not "finished". Subsequent
    edits land as follow-up PRs against the same file. The status field in the frontmatter tracks
    lifecycle; PR state is just how edits get in.
  </li>
</ol>

<h2>Status values</h2>
<ul>
  <li><code>Draft</code>: the proposal exists and is open to feedback. Default for a freshly merged RFC.</li>
  <li><code>Accepted</code>: the design is the plan of record. Implementation may not be started.</li>
  <li>
    <code>Implemented</code>: the proposal landed and the relevant code, docs, or process exist.
    Link the tracking issue and the PRs.
  </li>
  <li><code>Rejected</code>: a follow-up PR set this status. Keep the file so the reasoning is preserved.</li>
  <li><code>Withdrawn</code>: the author no longer pursues it. Same retention rule.</li>
  <li><code>Superseded</code>: pointed at a newer RFC via <code>supersededBy</code> in frontmatter.</li>
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
  the work lands, a follow-up PR sets the RFC status to <code>Implemented</code> and links the
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
    padding-left: 1.4rem;
  }

  .rfc-index li {
    margin: 0.3rem 0;
  }

  .status {
    color: var(--fg-muted);
    font-size: 0.9em;
  }
</style>
