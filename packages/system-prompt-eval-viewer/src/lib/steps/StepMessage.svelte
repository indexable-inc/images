<script lang="ts">
  let { text, final = false }: { text?: string; final?: boolean } = $props();
</script>

<div class="msg">
  <div class="lbl">{final ? 'final answer' : 'assistant'}</div>
  <div class="prose" class:final>{text}</div>
</div>

<style>
  .lbl { font-size: 11px; color: var(--dim); text-transform: uppercase; letter-spacing: 0.04em; margin-bottom: 3px; }
  .prose { white-space: pre-wrap; line-height: 1.55; }
  .prose.final { background: var(--good-bg); border: 1px solid color-mix(in oklab, var(--good) 30%, transparent);
    border-radius: var(--radius); padding: 10px 12px; }
</style>
