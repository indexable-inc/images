<script lang="ts">
  import type { RunInfo } from "../types";
  let { run }: { run: RunInfo } = $props();
  const name = $derived(run.run_id.replace(/-\d+.*$/, ""));
</script>

<a class="chip {run.status}" href="http://127.0.0.1:4040/ir/{run.run_id}" title={run.run_id}>
  <span class="name">{name}</span>
  <span class="status">{run.status}</span>
</a>

<style>
  .chip { display: inline-flex; gap: 0.45rem; align-items: baseline; border: 1px solid var(--border);
          padding: 0.25rem 0.6rem; text-decoration: none; color: inherit; font-size: 0.85rem; }
  .status { color: var(--muted); font-size: 0.75rem; }
  .failed .status { color: #e05252; }
  .running .status { color: #c9a45a; }
  .succeeded .status { color: #3fb96f; }
</style>
