<script lang="ts">
  let { label, value }: { label: string; value?: unknown } = $props();
</script>

<span class="chip"><b>{label}</b>{#if value !== undefined}<span class="v">{value === null ? '—' : String(value)}</span>{/if}</span>

<style>
  .chip { display: inline-flex; gap: 6px; align-items: baseline; font-size: 12px; background: var(--chip);
    border-radius: 4px; padding: 3px 9px; color: var(--dim); }
  b { color: var(--text); font-weight: 600; }
</style>
