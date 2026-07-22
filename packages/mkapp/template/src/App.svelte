<script lang="ts">
  import AgentStatus from './lib/AgentStatus.svelte';
  import Button from './lib/components/ui/Button.svelte';
  import Card from './lib/components/ui/Card.svelte';
  import Skeleton from './lib/components/ui/Skeleton.svelte';
  import { app } from './lib/store.svelte';

  function addNote() {
    app.sections.push({
      id: `note-${app.sections.length}`,
      title: 'Note',
      loading: false,
      body: 'Added in the browser; durable across hot reloads.',
    });
  }
</script>

<AgentStatus />

<main>
  {#each app.sections as section (section.id)}
    <Card title={section.title}>
      {#if section.loading}
        <Skeleton width="92%" />
        <Skeleton width="70%" />
        <Skeleton width="82%" />
      {:else}
        <p>{section.body}</p>
      {/if}
    </Card>
  {/each}

  <footer>
    <Button variant="outline" onclick={addNote}>Add section</Button>
  </footer>
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin-top: 12px;
  }
  p {
    margin: 0;
    white-space: pre-wrap;
  }
  footer {
    margin-top: 8px;
  }
</style>
