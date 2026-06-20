<script lang="ts">
  import CodeBlock from '../ui/CodeBlock.svelte';
  let { name, input }: { name?: string; input?: string } = $props();
</script>

<div class="head">tool <b>{name}</b></div>
{#if input}<CodeBlock text={input} variant="code" />{/if}

<style>
  .head { font-size: 12px; color: var(--dim); margin-bottom: 2px; }
  b { color: var(--accent); font-weight: 600; }
</style>
