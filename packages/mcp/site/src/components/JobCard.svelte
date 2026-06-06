<script lang="ts">
  import type { Job } from '$lib/types';
  import { now } from '$lib/now.svelte';
  import { duration } from '$lib/format';
  import StatusChip from './StatusChip.svelte';
  import RichOutput from './RichOutput.svelte';

  let { job }: { job: Job } = $props();

  // A running job's elapsed time tracks the shared clock; a finished one is fixed.
  const elapsed = $derived(duration((job.ended_at ?? now.value) - job.started_at));
  const hasRich = $derived(job.outputs.length > 0);
  // Don't repeat the error if it's already in the captured stdout tail.
  const showError = $derived(!!job.error && !(job.output ?? '').includes(job.error));
</script>

<article class="job {job.status}">
  <header class="hdr">
    <StatusChip status={job.status} />
    <span class="id">{job.id}</span>
    <span class="name">{job.name}</span>
    <span class="dur">{elapsed}</span>
  </header>

  <details class="code">
    <summary></summary>
    <pre>{job.code}</pre>
  </details>

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
    flex-wrap: wrap;
    gap: 9px;
    align-items: center;
  }
  .id {
    color: var(--muted);
    font-size: 12px;
  }
  .name {
    overflow: hidden;
    color: var(--text);
    font-size: 12px;
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
  .code {
    margin: 8px 0 0;
  }
  .code > summary {
    display: inline-block;
    color: var(--faint);
    font-size: 10px;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    list-style: none;
    cursor: pointer;
    user-select: none;
  }
  .code > summary::-webkit-details-marker {
    display: none;
  }
  .code > summary::before {
    content: '+ source';
  }
  .code[open] > summary::before {
    content: '\2212 source';
  }
  .code > pre {
    color: var(--dim);
    background: var(--inset);
    border: 1px solid var(--line);
    padding: 9px 11px;
  }
  pre {
    margin: 8px 0 0;
    max-height: 340px;
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
