<script lang="ts">
  import type { JobStatus } from '$lib/types';
  let { status }: { status: JobStatus } = $props();
</script>

<!-- A finished run is the uninteresting default: show a chip only for the
     states worth a glance (running, error, cancelled), nothing for done. -->
{#if status !== 'done'}
  <span class="chip {status}">{status}</span>
{/if}

<style>
  .chip {
    padding: 2px 6px;
    border: 1px solid var(--line-2);
    color: var(--dim);
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }
  .running {
    background: var(--active);
    color: var(--on-active);
    border-color: var(--active);
  }
  .error {
    color: var(--err);
    border-color: var(--err-line);
  }
  .cancelled,
  .interrupted {
    color: var(--muted);
  }
</style>
