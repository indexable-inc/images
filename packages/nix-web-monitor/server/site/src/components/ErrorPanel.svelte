<script lang="ts">
  /// Surfaces the actual error text Nix emitted. The summary bar only shows a
  /// count; without this panel the operator has to hunt the red lines out of
  /// the log stream. Clicking an error focuses the log view on it.

  type Props = {
    errors: string[];
    onclose: () => void;
    oninspect: (text: string) => void;
  };

  const { errors, onclose, oninspect }: Props = $props();

  /// Cap the rendered rows: an error-heavy eval can produce thousands, and the
  /// newest are the ones the operator acts on.
  const MAX_ROWS = 50;

  const recent = $derived.by(() => {
    const start = Math.max(0, errors.length - MAX_ROWS);
    return errors
      .slice(start)
      .map((text, offset) => ({ key: start + offset, text }))
      .toReversed();
  });
  const overflow = $derived(errors.length - recent.length);

  function firstLine(text: string): string {
    const newline = text.indexOf('\n');
    return (newline === -1 ? text : text.slice(0, newline)).trim();
  }
</script>

<section class="error-panel" role="alert" aria-label="errors">
  <div class="error-head">
    <span class="error-title">
      {String(errors.length)} error{errors.length === 1 ? '' : 's'}
    </span>
    {#if overflow > 0}
      <span class="error-overflow">showing last {String(recent.length)}</span>
    {/if}
    <button type="button" class="chip error-dismiss" onclick={onclose}>dismiss &times;</button>
  </div>
  <ul class="error-list">
    {#each recent as item (item.key)}
      <li>
        <button
          type="button"
          class="error-item"
          title="show in logs"
          onclick={() => {
            oninspect(firstLine(item.text));
          }}
        >
          {item.text}
        </button>
      </li>
    {/each}
  </ul>
</section>
