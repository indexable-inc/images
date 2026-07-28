<script lang="ts">
  import type { Eval } from './types';
  import Score from './ui/Score.svelte';
  let { name, ev }: { name: string; ev: Eval } = $props();
</script>

<a class="card" href={`#${name}`}>
  <div class="cn">{name}</div>
  <Score value={ev.headline} size="lg" />
  {#if ev.longest_streak != null}<div class="cs">streak {ev.longest_streak}</div>{/if}
</a>

<style>
  .card { text-decoration: none; color: inherit; border: 1px solid var(--line); border-radius: var(--radius);
    padding: 14px 16px; background: var(--card); transition: background 0.12s; display: block; }
  .card:hover { background: var(--panel); }
  .cn { font-size: 13px; color: var(--dim); margin-bottom: 2px; }
  .cs { font-size: 12px; color: var(--dim); }
</style>
