<script lang="ts">
  import { pct, grade } from '../types';
  let { value, size = 'md' }: { value: number; size?: 'sm' | 'md' | 'lg' } = $props();
</script>

<span class="score {grade(value)} {size}">{pct(value)}</span>

<style>
  .score { font-weight: 700; letter-spacing: -0.02em; }
  .sm { font-size: 14px; }
  .md { font-size: 26px; }
  .lg { font-size: 30px; }
  .good { color: var(--good); }
  .warn { color: var(--warn); }
  .bad { color: var(--bad); }
</style>
