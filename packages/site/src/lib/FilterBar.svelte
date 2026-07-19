<script lang="ts">
  import { highlightExpression } from './filter-expression';

  type Props = {
    value: string;
    onChange: (next: string) => void;
    matchCount: number;
    totalCount: number;
    error?: string;
  };

  const { value, onChange, matchCount, totalCount, error }: Props = $props();

  const tokens = $derived(highlightExpression(value));

  // Keep the highlighted overlay aligned with the input when the text scrolls
  // horizontally (long expressions). The overlay is `overflow: hidden`; this
  // shifts it to mirror the input's `scrollLeft`.
  let inputEl: HTMLInputElement | undefined = $state();
  let overlayEl: HTMLDivElement | undefined = $state();
  function syncScroll(): void {
    if (inputEl && overlayEl) {
      overlayEl.scrollLeft = inputEl.scrollLeft;
    }
  }
</script>

<section class="filter" aria-label="Filter entries by tag">
  <div class="row">
    <span class="prompt" aria-hidden="true">filter</span>
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
          onChange(event.currentTarget.value);
        }}
        onscroll={syncScroll}
        onkeyup={syncScroll}
        spellcheck="false"
        autocapitalize="off"
        autocomplete="off"
        autocorrect="off"
        placeholder="nix & (rust | zig)"
        aria-label="Filter expression"
      />
    </div>
    <span class="count" aria-live="polite">
      {matchCount} / {totalCount}
    </span>
  </div>
  {#if error}
    <p class="error" role="status">{error}</p>
  {/if}
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

  /* The overlay paints the syntax-highlighted text. The input above it has
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

  .tok-tag {
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

  .tok-error {
    color: var(--syntax-error);
    text-decoration: underline wavy;
    text-decoration-thickness: 1px;
  }

  .count {
    color: var(--fg-faint);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .error {
    margin: 0;
    padding: 0 2ch;
    color: var(--syntax-error);
  }
</style>
