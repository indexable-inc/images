<script lang="ts">
  // Cmd-K command palette. Combines the static command registry
  // (commands.ts) with the live thread list so a single ⌘K can
  // either run an action or jump to a chat.

  import { tick } from 'svelte';
  import { paletteOpen, closePalette } from '$lib/ui';
  import { paletteCommands, type CommandDef } from '$lib/commands';
  import { mergedThreadsList, type ServerThread } from '$lib/store';
  import { router } from '$lib/router';

  type Row =
    | { kind: 'command'; cmd: CommandDef }
    | { kind: 'thread'; thread: ServerThread };

  let open = $state(false);
  const unsubOpen = paletteOpen.subscribe((v) => (open = v));
  $effect(() => () => unsubOpen());

  let threads = $state<ServerThread[]>([]);
  const unsubT = mergedThreadsList.subscribe((v) => (threads = v));
  $effect(() => () => unsubT());

  let query = $state('');
  let focusIdx = $state(0);
  let inputEl: HTMLInputElement | undefined = $state();

  // Reset state every time the palette opens.
  $effect(() => {
    if (open) {
      query = '';
      focusIdx = 0;
      void tick().then(() => inputEl?.focus());
    }
  });

  let rows = $derived(buildRows(query, threads));

  function buildRows(q: string, ts: ServerThread[]): Row[] {
    const needle = q.trim().toLowerCase();
    const cmds = paletteCommands();
    const matchCmd = (c: CommandDef) =>
      !needle || c.label.toLowerCase().includes(needle);
    const matchThread = (t: ServerThread) =>
      !needle ||
      (t.title || 'Untitled').toLowerCase().includes(needle) ||
      (t.preview || '').toLowerCase().includes(needle) ||
      t.user.toLowerCase().includes(needle);

    const out: Row[] = [];
    for (const c of cmds) if (matchCmd(c)) out.push({ kind: 'command', cmd: c });
    for (const t of ts) if (matchThread(t)) out.push({ kind: 'thread', thread: t });
    return out;
  }

  function exec(row: Row) {
    closePalette();
    if (row.kind === 'command') {
      row.cmd.run();
    } else {
      router.go(
        '/s/' +
          encodeURIComponent(row.thread.server_id) +
          '/t/' +
          encodeURIComponent(row.thread.id)
      );
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      closePalette();
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      focusIdx = Math.min(focusIdx + 1, Math.max(rows.length - 1, 0));
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      focusIdx = Math.max(focusIdx - 1, 0);
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      const row = rows[focusIdx];
      if (row) exec(row);
    }
  }

  // Keep focusIdx valid as rows shrink while typing.
  $effect(() => {
    if (focusIdx >= rows.length) focusIdx = Math.max(rows.length - 1, 0);
  });
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={closePalette}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="palette" onclick={(e) => e.stopPropagation()}>
      <input
        bind:this={inputEl}
        bind:value={query}
        onkeydown={onKey}
        class="palette-input"
        placeholder="Type a command or search threads…"
        spellcheck="false"
        autocomplete="off"
      />

      <div class="palette-list">
        {#if rows.length === 0}
          <div class="palette-empty">No matches.</div>
        {:else}
          {#each rows as row, i (row.kind === 'command' ? `c:${row.cmd.id}` : `t:${row.thread.server_id}:${row.thread.id}`)}
            <!-- svelte-ignore a11y_mouse_events_have_key_events -->
            <button
              class="palette-row"
              class:focused={i === focusIdx}
              onmousemove={() => (focusIdx = i)}
              onclick={() => exec(row)}
            >
              {#if row.kind === 'command'}
                <span class="palette-label">{row.cmd.label}</span>
                {#if row.cmd.shortcut}
                  <span class="palette-meta">
                    <kbd>{row.cmd.shortcut}</kbd>
                  </span>
                {/if}
              {:else}
                <span class="palette-label">
                  {row.thread.title || 'Untitled'}
                </span>
                <span class="palette-meta palette-meta-thread">
                  <span class="palette-thread-user">{row.thread.user}</span>
                </span>
              {/if}
            </button>
          {/each}
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background: rgba(0, 0, 0, 0.30);
    backdrop-filter: blur(6px);
    -webkit-backdrop-filter: blur(6px);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 13vh;
  }

  .palette {
    width: 580px;
    max-width: calc(100vw - 32px);
    background: var(--bg-elev);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-popover);
    overflow: hidden;
    display: flex;
    flex-direction: column;
    max-height: 60vh;
  }

  .palette-input {
    border: none;
    outline: none;
    background: transparent;
    padding: 14px 18px;
    font-size: 15px;
    color: var(--text-strong);
    border-bottom: 1px solid var(--border);
  }
  .palette-input::placeholder {
    color: var(--text-dim);
  }

  .palette-list {
    overflow-y: auto;
    padding: 6px;
  }

  .palette-empty {
    padding: 22px 12px;
    text-align: center;
    color: var(--text-dim);
    font-size: 13px;
  }

  .palette-row {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    border-radius: var(--radius-sm);
    color: var(--text);
    font-size: 13px;
    text-align: left;
  }
  .palette-row.focused {
    background: var(--bg-active);
    color: var(--text-strong);
  }
  .palette-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    font-weight: 500;
  }
  .palette-meta {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text-muted);
    font-size: 11.5px;
    flex-shrink: 0;
  }
  .palette-meta kbd {
    font-family: var(--font-sans);
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--bg-pill);
    color: var(--text-muted);
  }
  .palette-thread-user {
    color: var(--text-dim);
  }
</style>
