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
  .filter {
    margin-bottom: 2.5rem;
    padding-bottom: 1.75rem;
    border-bottom: 1px solid var(--rule);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    font-family: var(--font-mono);
    font-size: 0.8125rem;
  }

  .prompt {
    color: var(--fg-faint);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .field {
    flex: 1;
    min-width: 0;
    position: relative;
    border: 1px solid var(--rule);
    border-radius: 4px;
    background: var(--bg);
    transition: border-color 0.15s ease;
  }

  .field:focus-within {
    border-color: var(--fg-muted);
  }

  /* The overlay paints the syntax-highlighted text. The input above it has
   * transparent text and a visible caret, so users see the highlighted
   * version of what they type. Padding, font, and line-height must match
   * pixel-for-pixel between the two layers. */
  .overlay,
  input {
    padding: 0.45rem 0.6rem;
    font-family: var(--font-mono);
    font-size: 0.8125rem;
    line-height: 1.4;
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
    color: light-dark(#0a7484, #5ec5d5);
    font-weight: 600;
  }

  .tok-op-or {
    color: light-dark(#a8651e, #e0a467);
    font-weight: 600;
  }

  .tok-op-not {
    color: light-dark(#7a3e8f, #c89be0);
    font-weight: 600;
  }

  .tok-paren {
    color: var(--fg-faint);
  }

  .tok-error {
    color: light-dark(#b91c1c, #fb7185);
    text-decoration: underline wavy;
    text-decoration-thickness: 1px;
  }

  .count {
    color: var(--fg-faint);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .error {
    margin-top: 0.5rem;
    font-family: var(--font-mono);
    font-size: 0.75rem;
    color: var(--fg-muted);
  }

</style>
