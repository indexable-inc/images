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
</script>

<article class="job {job.status}">
  <header class="hdr">
    <StatusChip status={job.status} />
    {#if title}<span class="name">{title}</span>{/if}
    <span class="dur">{elapsed}</span>
  </header>

  {#if job.code_html}
    <CodeView html={job.code_html} bindings={job.bindings} />
  {:else if job.code}
    <pre class="code">{job.code}</pre>
  {/if}

  {#if job.output}
    <pre class="out">{job.output}</pre>
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
  .hdr {
    display: flex;
    gap: 9px;
    align-items: baseline;
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
