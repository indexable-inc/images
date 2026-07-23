<script lang="ts">
  function setAll(open: boolean) {
    document.querySelectorAll<HTMLDetailsElement>('details.rollout').forEach((d) => (d.open = open));
  }
</script>

<div class="bar">
  <button onclick={() => setAll(true)}>expand all</button>
  <button onclick={() => setAll(false)}>collapse all</button>
  <span class="hint">click a run for its full action timeline · click a behavior dot to jump to a run</span>
</div>

<style>
  .bar { position: sticky; top: 0; z-index: 5; display: flex; align-items: center; gap: 8px;
    padding: 10px 0; margin-bottom: 8px; background: var(--bg); border-bottom: 1px solid var(--line); }
  button { font: inherit; font-size: 12px; border: 1px solid var(--line-strong); border-radius: var(--radius);
    padding: 4px 11px; background: var(--card); color: var(--text); cursor: pointer; }
  button:hover { background: var(--panel); }
  .hint { color: var(--dim); font-size: 12px; }
</style>
