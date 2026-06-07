<script lang="ts">
  import type { Job } from '$lib/types';
  import { now } from '$lib/now.svelte';
  import { duration, jobTitle } from '$lib/format';
  import StatusChip from './StatusChip.svelte';
  import RichOutput from './RichOutput.svelte';
  import CodeView from './CodeView.svelte';

  let { job }: { job: Job } = $props();

  // Only an explicit caller name labels the card; the source is shown below.
  const title = $derived(jobTitle(job.name, job.id));
  // A running job's elapsed time tracks the shared clock; a finished one is fixed.
  const elapsed = $derived(duration((job.ended_at ?? now.value) - job.started_at));
  const hasRich = $derived(job.outputs.length > 0);
  // Don't repeat the error if it's already in the captured stdout tail.
  const showError = $derived(!!job.error && !(job.output ?? '').includes(job.error));
  // A run reads as its results by default: the source and the captured stdout are
  // one click away, not in the way (stdout is not the channel — a run reports
  // through Results). Only runs that carry source or stdout get a reveal toggle.
  const hasDetails = $derived(!!(job.code_html || job.code || job.output));
  let showDetails = $state(false);
</script>

<article class="job {job.status}">
  <!-- The header doubles as the source toggle: a caret on the left reveals the
       code, the rest stays a quiet label. Inert when there is no source. -->
  <button
    class="hdr"
    type="button"
    aria-expanded={showDetails}
    disabled={!hasDetails}
    title={hasDetails ? (showDetails ? 'Hide source & output' : 'Show source & output') : undefined}
    onclick={() => (showDetails = !showDetails)}
  >
    {#if hasDetails}<span class="caret" aria-hidden="true">{showDetails ? '▾' : '▸'}</span>{/if}
    <StatusChip status={job.status} />
    {#if title}<span class="name">{title}</span>{/if}
    <span class="dur">{elapsed}</span>
  </button>

  {#if showDetails}
    {#if job.code_html}
      <CodeView html={job.code_html} bindings={job.bindings} />
    {:else if job.code}
      <pre class="code">{job.code}</pre>
    {/if}
    {#if job.output}
      <pre class="out">{job.output}</pre>
    {/if}
  {/if}

  {#if hasRich}
    {#each job.outputs as output, i (i)}
      <RichOutput {output} />
    {/each}
  {:else if job.result}
    <pre class="res">{job.result}</pre>
  {/if}

  {#if showError}
    <pre class="err">{job.error}</pre>
  {/if}
</article>

<style>
  .job {
    margin: 0 0 9px;
    padding: 11px 14px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-left: 2px solid var(--line-2);
  }
  .job.running {
    border-left-color: var(--active);
  }
  .job.error {
    border-left-color: var(--err);
  }
  /* The header is the source toggle, so it is a full-width button reset to read
     as the plain status line it replaces. */
  .hdr {
    appearance: none;
    width: 100%;
    display: flex;
    gap: 9px;
    align-items: baseline;
    margin: 0;
    padding: 0;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .hdr:disabled {
    cursor: default;
  }
  .hdr:focus-visible {
    outline: 1px solid var(--active);
    outline-offset: 2px;
  }
  .caret {
    flex: none;
    color: var(--faint);
    font-size: 9px;
    line-height: 1;
    align-self: center;
  }
  .hdr:hover .caret {
    color: var(--active);
  }
  .name {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    color: var(--text);
    font-size: 12.5px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dur {
    flex: none;
    margin-left: auto;
    color: var(--faint);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  /* The source, syntax-highlighted (inline monokai spans from the server). Sits
     quietly under the header: the colored tokens carry the meaning, so the box
     itself stays flat and dim. */
  pre.code {
    margin: 8px 0 0;
    padding: 9px 11px;
    background: var(--inset);
    border: 1px solid var(--line);
  }
  /* Output shows in full; the column scrolls, not each card, so you are not
     trapped scrolling a 340px box inside a box. A runaway stream is still capped
     generously so one job cannot push everything else off-screen. */
  pre {
    margin: 8px 0 0;
    max-height: 70vh;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
    font-size: 12px;
    color: var(--dim);
  }
  pre.out {
    color: var(--dim);
  }
  pre.res {
    color: var(--text);
  }
  pre.err {
    color: var(--err);
  }
</style>
