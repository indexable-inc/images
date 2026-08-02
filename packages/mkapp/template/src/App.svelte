<script lang="ts">
  import AgentStatus from '$lib/AgentStatus.svelte';
  import History from '$lib/History.svelte';
  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import { Skeleton } from '$lib/components/ui/skeleton';
  import { add, by, history, toggleHistory, viewAt, viewState, viewing } from '$lib/store.svelte';

  // The state to render: the live store, or a reconstruction of a past one when
  // a history row is selected. Everything below reads `view` rather than `app`,
  // so time travel needs no second set of components.
  const view = $derived(viewState());
  const past = $derived(viewing.seq !== null);

  function addNote() {
    // Through the mutator, not by pushing into the store: a direct write would
    // bypass the log and land in the history as an anonymous `external` change.
    // Attributed to the person at the browser, which is what `by` is for.
    by('you', 'human');
    add({
      id: `note-${Date.now()}`,
      title: 'Note',
      loading: false,
      body: 'Added in the browser; durable across hot reloads, and in the history.',
    });
  }
</script>

<History />

<div class="transition-[padding]" class:pr-[286px]={history.open}>
  {#if past}
    <!-- The page is showing a past state. Said plainly and at the top, because a
         reader who has forgotten they scrubbed would otherwise read a stale page
         as the current one. -->
    <div
      class="mb-3 flex items-center gap-2 rounded-lg border border-dashed px-3.5 py-2 font-mono text-xs"
      role="status"
    >
      <span class="text-muted-foreground">showing this page as of an earlier change</span>
      <Button class="ml-auto h-6 px-2 text-[11px]" variant="outline" onclick={() => viewAt(null)}>
        back to live
      </Button>
    </div>
  {/if}

  <AgentStatus state={view} />

  <main class="mt-3 flex flex-col gap-3">
    {#each view.sections as section (section.id)}
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

    <footer class="mt-2 flex items-center gap-2">
      <Button variant="outline" onclick={addNote} disabled={past}>Add section</Button>
      <Button variant="ghost" onclick={() => toggleHistory()}>
        History <kbd class="ml-1.5 font-mono text-[10px] text-muted-foreground">H</kbd>
      </Button>
    </footer>
  </main>
</div>
