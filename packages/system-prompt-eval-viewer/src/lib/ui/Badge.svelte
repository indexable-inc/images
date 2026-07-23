<script lang="ts">
  let { kind, text }: { kind: 'good' | 'warn' | 'bad'; text: string } = $props();
</script>

<span class="badge {kind}">{text}</span>

<style>
  .badge { min-width: 54px; text-align: center; border-radius: 4px; padding: 2px 9px; font-weight: 600; font-size: 12px; }
  .good { background: var(--good-bg); color: var(--good-fg); }
  .warn { background: var(--warn-bg); color: var(--warn-fg); }
  .bad { background: var(--bad-bg); color: var(--bad-fg); }
</style>
