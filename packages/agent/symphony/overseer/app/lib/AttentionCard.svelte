<script lang="ts">
  import type { AttentionItem } from "../types";
  let { item }: { item: AttentionItem } = $props();
</script>

<div class="card {item.severity}">
  <div class="head">
    <span class="sev">{item.severity === "fix" ? "needs fix" : "watch"}</span>
    <span class="title">{item.title}</span>
  </div>
  <p class="why">{item.why}</p>
  {#if item.action}
    <p class="action">{item.action}</p>
  {/if}
  {#if item.dispatched}
    <p class="dispatched">→ {item.dispatched}</p>
  {/if}
</div>

<style>
  .card { border: 1px solid var(--border); border-left-width: 3px; padding: 0.7rem 0.9rem; }
  .card.fix { border-left-color: #e05252; }
  .card.watch { border-left-color: #c9a45a; }
  .head { display: flex; gap: 0.6rem; align-items: baseline; }
  .sev { font-size: 0.7rem; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); }
  .fix .sev { color: #e05252; }
  .watch .sev { color: #c9a45a; }
  .title { font-weight: 600; }
  .why { margin: 0.35rem 0 0; color: var(--fg); }
  .action { margin: 0.35rem 0 0; font-family: var(--mono); font-size: 0.85em; color: var(--muted); }
  .dispatched { margin: 0.35rem 0 0; font-size: 0.85em; color: #3fb96f; }
</style>
