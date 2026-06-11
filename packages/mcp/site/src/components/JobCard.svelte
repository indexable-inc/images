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
  const elapsedSec = $derived((job.ended_at ?? now.value) - job.started_at);
  const elapsed = $derived(duration(elapsedSec));
  // While running, a blue bar fills over the foreground budget (the "total
  // timeout" before the call backgrounds). Once elapsed passes the budget the
  // job has been backgrounded but is still running: the bar reads as full and
  // shimmers to mark it as over budget.
  const frac = $derived(job.budget > 0 ? Math.min(1, elapsedSec / job.budget) : 0);
  const overBudget = $derived(elapsedSec >= job.budget);
  const hasRich = $derived(job.outputs.length > 0);
  const hasDetails = $derived(!!(job.code_html || job.code || job.output));

  // A run reads as its results by default, but the *active* states open
  // themselves: a running job shows its source with the executing line
  // highlighted live, and a failed one shows the line it failed on. A user
  // click takes over from the automatic behaviour for this card.
  let userToggled = $state<boolean | null>(null);
  const autoOpen = $derived(job.status === 'running' || job.status === 'error');
  const showDetails = $derived(hasDetails && (userToggled ?? autoOpen));

  // The kernel appends the error text to the captured stdout (so the model can
  // page it); the card shows the error in its own panel, so strip that trailing
  // copy from the stdout block rather than printing it twice.
  const outText = $derived.by(() => {
    const out = job.output ?? '';
    if (job.error && out.endsWith(job.error)) return out.slice(0, out.length - job.error.length).trimEnd();
    return out;
  });

  // Error presentation: a Python traceback gets a headline (the exception line,
  // e.g. "ValueError: boom") and a "line N" badge above the full text; a plain
  // contract message (no frames) renders as-is.
  const isTraceback = $derived(!!job.error && /^\s*(Traceback \(most recent call last\):|File ")/.test(job.error));
  const errHeadline = $derived.by(() => {
    if (!job.error || !isTraceback) return '';
    const lines = job.error.split('\n');
    for (let i = lines.length - 1; i >= 0; i--) {
      // The exception line of a traceback: "TypeError: ...", "fff.FffError: ...".
      if (/^[A-Za-z_][\w.]*(?::\s|$)/.test(lines[i])) return lines[i].trim();
    }
    return '';
  });
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
    onclick={() => (userToggled = !showDetails)}
  >
    {#if hasDetails}<span class="caret" aria-hidden="true">{showDetails ? '▾' : '▸'}</span>{/if}
    <StatusChip status={job.status} />
    {#if title}<span class="name">{title}</span>{:else}<span class="name"></span>{/if}
    {#if job.status === 'running' && job.line != null}
      <span class="at" title="executing line {job.line} of this cell">line {job.line}</span>
    {/if}
    <span class="dur">{elapsed}</span>
  </button>

  {#if job.status === 'running'}
    <div
      class="budget {overBudget ? 'over' : ''}"
      role="progressbar"
      aria-valuemin="0"
      aria-valuemax={job.budget}
      aria-valuenow={Math.min(elapsedSec, job.budget)}
      title={`${Math.round(elapsedSec)}s / ${job.budget}s`}
    >
      <div class="fill" style:width="{frac * 100}%"></div>
    </div>
  {/if}

  {#if showDetails}
    {#if job.code_html}
      <CodeView
        html={job.code_html}
        bindings={job.bindings}
        currentLine={job.status === 'running' ? job.line : null}
        errorLine={job.status === 'error' ? job.error_line : null}
      />
    {:else if job.code}
      <pre class="code">{job.code}</pre>
    {/if}
    {#if outText}
      <pre class="out">{outText}</pre>
    {/if}
  {/if}

  {#if hasRich}
    {#each job.outputs as output, i (i)}
      <RichOutput {output} />
    {/each}
  {:else if job.result}
    <pre class="res">{job.result}</pre>
  {/if}

  {#if job.error}
    <div class="errbox">
      {#if errHeadline || job.error_line != null}
        <div class="errhd">
          <span class="errmark" aria-hidden="true">✕</span>
          <span class="errname">{errHeadline || 'error'}</span>
          {#if job.error_line != null}
            <span class="errat" title="raised on line {job.error_line} of this cell">line {job.error_line}</span>
          {/if}
        </div>
      {/if}
      <pre class="err">{job.error}</pre>
    </div>
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
  /* The live "line N" readout: where the running cell is right now. Quiet and
     tabular so the number ticking up does not jitter the header. */
  .at {
    flex: none;
    color: var(--accent);
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
  }
  .dur {
    flex: none;
    margin-left: auto;
    color: var(--faint);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  /* A blue bar tracking elapsed-vs-budget for a running job: how much of the
     foreground "total timeout" has been spent before the call backgrounds. */
  .budget {
    margin: 7px 0 1px;
    height: 3px;
    background: var(--inset);
    border-radius: 2px;
    overflow: hidden;
  }
  .budget .fill {
    height: 100%;
    background: var(--accent);
    border-radius: inherit;
    /* Smooth the jump between the page's one-second clock ticks. */
    transition: width 1s linear;
  }
  /* Past budget the job is backgrounded but still running: hold the bar full and
     sweep a highlight across it so it reads as ongoing, not stalled. */
  .budget.over .fill {
    transition: none;
    background-image: linear-gradient(
      90deg,
      var(--accent) 0%,
      color-mix(in srgb, var(--accent) 45%, transparent) 50%,
      var(--accent) 100%
    );
    background-size: 200% 100%;
    animation: budget-sweep 1.1s linear infinite;
  }
  @keyframes budget-sweep {
    from {
      background-position: 200% 0;
    }
    to {
      background-position: -200% 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .budget .fill {
      transition: none;
    }
    .budget.over .fill {
      animation: none;
    }
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

  /* The error panel: the exception headline and the failing line up front, the
     full (cell-trimmed) traceback beneath. */
  .errbox {
    margin: 8px 0 0;
    border: 1px solid var(--err-line);
    background: color-mix(in srgb, var(--err) 5%, var(--inset));
  }
  .errhd {
    display: flex;
    gap: 8px;
    align-items: baseline;
    min-width: 0;
    padding: 6px 9px;
    border-bottom: 1px solid var(--err-line);
  }
  .errmark {
    flex: none;
    color: var(--err);
    font-size: 10px;
  }
  .errname {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    color: var(--err);
    font-size: 12px;
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .errat {
    flex: none;
    padding: 1px 6px;
    border: 1px solid var(--err-line);
    color: var(--err);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
  }
  pre.err {
    margin: 0;
    padding: 7px 9px;
    color: var(--err);
  }
</style>
