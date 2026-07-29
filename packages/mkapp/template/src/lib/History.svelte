<script lang="ts">
  // The version history: every change to this page, newest first.
  //
  // One row is one recorded mutation. It says WHO made it (the ambient actor the
  // update file declared with `by()`, so a relayed subagent is named rather than
  // the one process that wrote it), WHAT it did (the mutation's own label, which
  // is exact because it was captured at the call site rather than reconstructed
  // from a diff), and WHEN, as a bare relative age.
  //
  // Clicking a row shows the page as it stood just after that change. The page
  // is reconstructed by walking inverses backwards from the live state, so it is
  // the real past state and not an approximation of one. Clicking the marked row
  // again, or pressing Escape, returns to live.
  //
  // `H` toggles this panel, matching the document key set in test-ide rather
  // than inventing a binding.
  import { Button } from '$lib/components/ui/button';
  import { type Entry, shortAge } from '$lib/history';
  import { history, toggleHistory, viewAt, viewing } from '$lib/store.svelte';

  // Ages are relative, so they need a clock of their own: nothing else changes
  // when a row gets a minute older.
  let now = $state(Date.now());
  $effect(() => {
    const tick = setInterval(() => (now = Date.now()), 15_000);
    return () => clearInterval(tick);
  });

  const rows = $derived([...history.entries].reverse());
  const live = $derived(viewing.seq === null);

  function pick(entry: Entry): void {
    viewAt(viewing.seq === entry.seq ? null : entry.seq);
  }
</script>

<svelte:window
  onkeydown={(event) => {
    const target = event.target as HTMLElement | null;
    // Never steal a key from something being typed into.
    if (target?.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target?.tagName ?? '')) return;
    if (event.key === 'H') {
      event.preventDefault();
      toggleHistory();
    } else if (event.key === 'Escape' && !live) {
      viewAt(null);
    }
  }}
/>

{#if history.open}
  <aside
    class="fixed inset-y-0 right-0 z-40 flex w-[286px] flex-col border-l bg-card"
    aria-label="version history"
  >
    <header class="flex flex-none items-center gap-2 border-b px-3 py-2.5">
      <span class="font-mono text-[11px] tracking-[0.08em] text-muted-foreground uppercase">
        history
      </span>
      <span
        class="ml-auto rounded border px-1.5 font-mono text-[10px] tabular-nums text-muted-foreground"
      >
        {history.entries.length}
      </span>
      <button
        class="px-1 font-mono text-xs text-muted-foreground hover:text-foreground"
        aria-label="hide history"
        onclick={() => toggleHistory(false)}
      >
        ×
      </button>
    </header>

    {#if !live}
      <div class="flex flex-none items-center gap-2 border-b bg-accent/40 px-3 py-2">
        <span class="font-mono text-[10.5px] text-muted-foreground">viewing the past</span>
        <Button class="ml-auto h-6 px-2 text-[11px]" variant="outline" onclick={() => viewAt(null)}>
          back to live
        </Button>
      </div>
    {/if}

    <div class="min-h-0 flex-1 overflow-y-auto">
      {#each rows as entry (entry.seq)}
        <button
          class="block w-full border-b border-l-2 px-2.5 py-1.5 text-left hover:bg-accent/50"
          class:border-l-primary={viewing.seq === entry.seq}
          class:bg-accent={viewing.seq === entry.seq}
          class:border-l-transparent={viewing.seq !== entry.seq}
          onclick={() => pick(entry)}
          title={entry.target ? `${entry.label} — ${entry.target}` : entry.label}
        >
          <span class="flex items-baseline gap-2">
            <!-- Attribution reads as ink, not as a badge, and an entry is never
                 attributed to nobody: the reconciler synthesises a label when
                 the log has none. -->
            <span
              class="max-w-[76px] flex-none truncate font-mono text-[10.5px]"
              class:text-primary={entry.actor.kind === 'human'}
              class:text-muted-foreground={entry.actor.kind !== 'human'}
            >
              {entry.actor.label}
            </span>
            <span class="min-w-0 flex-1 truncate font-mono text-[11.5px] text-foreground">
              {entry.label}
            </span>
            <span class="flex-none font-mono text-[10.5px] tabular-nums text-muted-foreground">
              {shortAge(entry.ts, now)}
            </span>
          </span>
          {#if entry.kind === 'external'}
            <!-- A change that reached the state without a mutator. Shown rather
                 than hidden: a history that quietly omits a change is worse than
                 no history, because it looks complete. -->
            <span class="mt-0.5 block font-mono text-[10px] text-destructive">
              outside the update surface
            </span>
          {/if}
        </button>
      {/each}
    </div>
  </aside>
{/if}
