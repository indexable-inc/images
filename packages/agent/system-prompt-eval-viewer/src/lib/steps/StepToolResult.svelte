<script lang="ts">
  import CodeBlock from '../ui/CodeBlock.svelte';
  let { text, isError = false }: { text?: string; isError?: boolean } = $props();
</script>

<div class="head" class:err={isError}>{isError ? 'tool result · error' : 'tool result'}</div>
<CodeBlock text={text ?? ''} variant="out" />

<style>
  .head { font-size: 12px; color: var(--dim); margin-bottom: 2px; }
  .head.err { color: var(--bad); }
</style>
