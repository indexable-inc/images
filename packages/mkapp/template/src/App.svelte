<script lang="ts">
  import AgentStatus from '$lib/AgentStatus.svelte';
  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import { Skeleton } from '$lib/components/ui/skeleton';
  import { app } from '$lib/store.svelte';

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

<main class="mt-3 flex flex-col gap-3">
  {#each app.sections as section (section.id)}
    <Card.Root>
      <Card.Header>
        <Card.Title>{section.title}</Card.Title>
      </Card.Header>
      <Card.Content class="flex flex-col gap-2 text-sm text-muted-foreground">
        {#if section.loading}
          <Skeleton class="h-3.5 w-[92%]" />
          <Skeleton class="h-3.5 w-[70%]" />
          <Skeleton class="h-3.5 w-[82%]" />
        {:else}
          <p class="whitespace-pre-wrap">{section.body}</p>
        {/if}
      </Card.Content>
    </Card.Root>
  {/each}

  <footer class="mt-2">
    <Button variant="outline" onclick={addNote}>Add section</Button>
  </footer>
</main>
