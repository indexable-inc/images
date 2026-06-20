<script lang="ts">
  import { grade } from '../types';
  let { value }: { value: number } = $props();
</script>

<div class="bar"><span class={grade(value)} style="width:{Math.round(value * 100)}%"></span></div>

<style>
  .bar { height: 6px; border-radius: 999px; background: var(--chip); overflow: hidden; margin: 8px 0; }
  span { display: block; height: 100%; border-radius: 999px; }
  .good { background: var(--good); }
  .warn { background: var(--warn); }
  .bad { background: var(--bad); }
</style>
