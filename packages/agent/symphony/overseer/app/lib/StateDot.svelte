<script lang="ts">
  import type { AgentState } from "../types";
  let { state }: { state: AgentState } = $props();
</script>

<span class="dot {state}" title={state}></span>

<style>
  .dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%; flex: none; }
  .progressing { background: #3fb96f; }
  .waiting { background: #c9a45a; }
  .stuck { background: #e05252; }
  .idle { background: #55554f; }
</style>
