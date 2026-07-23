<script lang="ts">
  import CodeBlock from '../ui/CodeBlock.svelte';
  let { text }: { text?: string } = $props();
</script>

<details>
  <summary>thinking</summary>
  <CodeBlock text={text ?? ''} variant="think" />
</details>

<style>
  summary { cursor: pointer; color: var(--dim); font-size: 12px; }
</style>
