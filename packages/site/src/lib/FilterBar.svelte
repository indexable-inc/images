<script lang="ts">
  import { tick } from 'svelte';
  import { searchTokens, wordAt, type TagOption } from './filter-expression';

  type Props = {
    value: string;
    onChange: (next: string) => void;
    matchCount: number;
    totalCount: number;
    // Known tags with entry counts, e.g. from `tagOptions(siteUpdates)`.
    // Drives both the inline pill highlighting and the autocomplete menu.
    tags: readonly TagOption[];
  };

  const { value, onChange, matchCount, totalCount, tags }: Props = $props();

  const tagNames = $derived(tags.map((t) => t.name));
  const tokens = $derived(searchTokens(value, tagNames));

  // Keep the highlighted overlay aligned with the input when the text scrolls
  // horizontally (long queries). The overlay is `overflow: hidden`; this
  // shifts it to mirror the input's `scrollLeft`. The same offset repositions
  // the autocomplete menu so it stays under the word being completed.
  let inputEl: HTMLInputElement | undefined = $state();
  let overlayEl: HTMLDivElement | undefined = $state();
  let scrollLeft = $state(0);
  function syncScroll(): void {
    if (inputEl && overlayEl) {
      overlayEl.scrollLeft = inputEl.scrollLeft;
      scrollLeft = inputEl.scrollLeft;
    }
  }

  // Autocomplete completes the word under the caret. `caret` is a plain
  // number, so re-assigning an unchanged position (arrow-key navigation)
  // does not recompute `suggestions` and reset the selection.
  let caret = $state(0);
  let focused = $state(false);
  let dismissed = $state(false);
  let selected = $state(0);

  function syncCaret(): void {
    caret = inputEl?.selectionStart ?? value.length;
  }

  const word = $derived(wordAt(value, caret));
  const suggestions = $derived.by(() => {
    const prefix = word.word.toLowerCase();
    if (prefix.length === 0) return [];
    const hits = tags.filter((t) => t.name.startsWith(prefix));
    // The word already is the only completion: nothing left to offer.
    if (hits.length === 1 && hits[0]?.name === prefix) return [];
    return hits;
  });
  const open = $derived(focused && !dismissed && suggestions.length > 0);

  $effect(() => {
    void suggestions;
    selected = 0;
  });

  async function accept(name: string): Promise<void> {
    const { start, end } = wordAt(value, caret);
    const after = value.slice(end);
    const insert = after.startsWith(' ') ? name : `${name} `;
    const next = value.slice(0, start) + insert + after;
    // Land after the completed tag plus one space, ready for the next term.
    const nextCaret = start + name.length + 1;
    onChange(next);
    dismissed = false;
    await tick();
    inputEl?.setSelectionRange(nextCaret, nextCaret);
    caret = nextCaret;
  }

  function onKeydown(event: KeyboardEvent): void {
    if (!open) return;
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      selected = (selected + 1) % suggestions.length;
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      selected = (selected + suggestions.length - 1) % suggestions.length;
    } else if (event.key === 'Tab' || event.key === 'Enter') {
      event.preventDefault();
      void accept(suggestions[selected].name);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      dismissed = true;
    }
  }
</script>

<section class="filter" aria-label="Search entries">
  <div class="row">
    <span class="prompt" aria-hidden="true">search</span>
    <div class="field">
      <div class="overlay" bind:this={overlayEl} aria-hidden="true">
        {#each tokens as token, i (i)}<span class={`tok tok-${token.kind}`}
            >{token.text}</span
          >{/each}
      </div>
      <input
        bind:this={inputEl}
        type="text"
        {value}
        oninput={(event) => {
          dismissed = false;
          onChange(event.currentTarget.value);
          caret = event.currentTarget.selectionStart ?? event.currentTarget.value.length;
        }}
        onscroll={syncScroll}
        onkeydown={onKeydown}
        onkeyup={() => {
          syncScroll();
          syncCaret();
        }}
        onclick={syncCaret}
        onfocus={() => {
          focused = true;
          syncCaret();
        }}
        onblur={() => {
          focused = false;
        }}
        spellcheck="false"
        autocapitalize="off"
        autocomplete="off"
        autocorrect="off"
        placeholder="search titles, bodies, tags"
        role="combobox"
        aria-label="Search entries"
        aria-autocomplete="list"
        aria-expanded={open}
        aria-controls={open ? 'filter-suggestions' : undefined}
        aria-activedescendant={open ? `filter-option-${String(selected)}` : undefined}
      />
      {#if open}
        <ul
          class="menu"
          id="filter-suggestions"
          role="listbox"
          aria-label="Matching tags"
          style={`--word-ch: ${String(word.start)}; --scroll-px: ${String(scrollLeft)}px`}
        >
          {#each suggestions as suggestion, i (suggestion.name)}
            <!-- Selection follows the pointer; keyboard is handled on the
                 input per the combobox pattern, so focus never leaves it.
                 mousedown is prevented to keep the input focused on click. -->
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <li
              id={`filter-option-${String(i)}`}
              role="option"
              aria-selected={i === selected}
              class:selected={i === selected}
              onmousedown={(event) => {
                event.preventDefault();
              }}
              onclick={() => {
                void accept(suggestion.name);
              }}
              onmouseenter={() => {
                selected = i;
              }}
            >
              <span class="menu-name">{suggestion.name}</span>
              <span class="menu-count">{suggestion.count}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
    <span class="count" aria-live="polite">
      {matchCount} / {totalCount}
    </span>
  </div>
</section>

<style>
  /* A TUI panel: 1px frame with the title sitting inline in the top rule.
     Interior is exactly one cell tall; the frame's vertical 1px rules are
     absorbed into the paddings so following content stays on the grid. */
  .filter {
    position: relative;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    margin-bottom: calc(var(--cell-h) * 2);
  }

  .filter:focus-within {
    border-color: var(--fg-muted);
  }

  .prompt {
    position: absolute;
    top: calc(var(--cell-h) / -2);
    left: 2ch;
    padding: 0 1ch;
    background: var(--bg);
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    font-weight: 700;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 2ch;
    padding: calc(var(--cell-h) - 1px) 2ch;
  }

  .field {
    flex: 1;
    min-width: 0;
    position: relative;
  }

  /* The overlay paints the highlighted text. The input above it has
   * transparent text and a visible caret, so users see the highlighted
   * version of what they type. The two layers share the base cell exactly. */
  .overlay,
  input {
    padding: 0;
    font-family: var(--font-mono);
    letter-spacing: 0;
  }

  .overlay {
    position: absolute;
    inset: 0;
    pointer-events: none;
    white-space: pre;
    overflow: hidden;
    color: var(--fg);
  }

  input {
    position: relative;
    z-index: 1;
    display: block;
    width: 100%;
    border: 0;
    background: transparent;
    color: transparent;
    caret-color: var(--fg);
  }

  input:focus {
    outline: none;
  }

  input::placeholder {
    color: var(--fg-faint);
  }

  /* Recognized tags (and prefixes of them) read as subtle inline pills:
     muted ground, ink text, never an error treatment. The box-shadow fakes
     1px of pill padding without moving glyphs off the character grid. */
  .tok-tag {
    background: var(--rule);
    color: var(--fg);
    border-radius: var(--radius);
    box-shadow: 0 0 0 1px var(--rule);
  }

  /* Free text is a full-text query, styled as plain typing. */
  .tok-text {
    color: var(--fg);
  }

  .tok-op-and {
    color: var(--syntax-and);
    font-weight: 700;
  }

  .tok-op-or {
    color: var(--syntax-or);
    font-weight: 700;
  }

  .tok-op-not {
    color: var(--syntax-not);
    font-weight: 700;
  }

  .tok-paren {
    color: var(--fg-faint);
  }

  /* The autocomplete: a box-drawn menu one cell below the panel's bottom
     rule, left-aligned under the word being completed (the field is mono,
     so character index * 1ch is the pixel offset). */
  .menu {
    position: absolute;
    top: calc(var(--cell-h) * 2);
    left: clamp(0px, calc(var(--word-ch) * 1ch - var(--scroll-px, 0px)), calc(100% - 30ch));
    z-index: 10;
    margin: 0;
    padding: 0;
    list-style: none;
    min-width: 30ch;
    max-width: 100%;
    max-height: calc(var(--cell-h) * 8);
    overflow-y: auto;
    background: var(--bg);
    border: 1px solid var(--rule);
    border-radius: var(--radius);
  }

  .menu li {
    display: flex;
    justify-content: space-between;
    gap: 2ch;
    height: var(--cell-h);
    line-height: var(--cell-h);
    padding: 0 1ch;
    cursor: pointer;
    white-space: nowrap;
  }

  /* Inverse video marks the active row, TUI style. */
  .menu li.selected {
    background: var(--fg);
    color: var(--bg);
  }

  .menu-count {
    color: var(--fg-faint);
    font-variant-numeric: tabular-nums;
  }

  .menu li.selected .menu-count {
    color: var(--bg);
  }

  .count {
    color: var(--fg-faint);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
</style>
