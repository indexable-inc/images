<script lang="ts">
  let { text, variant = 'out' }: { text: string; variant?: 'code' | 'out' | 'think' } = $props();
  let copied = $state(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
      setTimeout(() => (copied = false), 1200);
    } catch {
      /* clipboard unavailable */
    }
  }
</script>

<div class="wrap">
  <button class="copy" onclick={copy} title="copy">{copied ? 'copied' : 'copy'}</button>
  <pre class={variant}>{text}</pre>
</div>

<style>
  .wrap { position: relative; }
  .copy { position: absolute; top: 6px; right: 6px; font: 11px var(--sans); color: var(--dim);
    background: var(--card); border: 1px solid var(--line); border-radius: 4px; padding: 1px 7px; cursor: pointer; opacity: 0; transition: opacity 0.12s; }
  .wrap:hover .copy { opacity: 1; }
  pre { margin: 4px 0 0; white-space: pre-wrap; word-break: break-word; border-radius: var(--radius);
    padding: 10px 12px; font: 12px/1.5 var(--mono); max-height: 380px; overflow: auto; border: 1px solid var(--line); }
  .code { background: var(--panel); }
  .out { background: var(--panel); color: var(--text); }
  .think { background: var(--panel); border-style: dashed; color: var(--dim); }
</style>
