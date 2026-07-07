<script lang="ts">
  import type { AgentJudgment } from "../types";
  import StateDot from "./StateDot.svelte";
  let { agent }: { agent: AgentJudgment } = $props();
</script>

<details class="agent">
  <summary>
    <StateDot state={agent.state} />
    <span class="label">{agent.label}</span>
    <span class="doing">{agent.doing}</span>
  </summary>
  <p class="why">{agent.why}</p>
</details>

<style>
  .agent { border-top: 1px solid var(--border); }
  summary { display: flex; align-items: baseline; gap: 0.6rem; padding: 0.45rem 0; cursor: pointer; list-style: none; }
  summary::-webkit-details-marker { display: none; }
  .label { font-weight: 600; white-space: nowrap; }
  .doing { color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .why { margin: 0 0 0.6rem 1.1rem; color: var(--muted); }
</style>
