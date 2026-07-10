<script lang="ts">
  // The keyboard cheatsheet, toggled by `?`. A quiet centered card grouping the
  // bindings the global keymap (lib/keys.svelte) owns. Click the backdrop or press
  // Esc/?/q to dismiss (the keymap handles the keys; the backdrop handles clicks).
  import { keys } from '$lib/keys.svelte';

  const groups: { title: string; rows: [string, string][] }[] = [
    {
      title: 'Move',
      rows: [
        ['j  k', 'down / up'],
        ['g g', 'top'],
        ['G', 'bottom'],
        ['^d  ^u', 'half page down / up'],
      ],
    },
    {
      title: 'Act',
      rows: [
        ['o  ⏎  l', 'open resource / rich output'],
        ['h', 'fold session'],
        ['z a', 'toggle fold'],
      ],
    },
    {
      title: 'Find',
      rows: [
        ['/', 'filter'],
        ['esc', 'close / clear'],
        ['?', 'this help'],
      ],
    },
  ];
</script>

{#if keys.help}
  <div class="kh-modal">
    <!-- A real button is the dismiss surface (keyboard-activatable, no a11y
         hacks); the card is a sibling above it, so card clicks never dismiss. -->
    <button class="kh-backdrop" aria-label="close help" onclick={() => (keys.help = false)}
    ></button>
    <div class="kh-card" role="dialog" aria-modal="true" aria-label="keyboard shortcuts" tabindex="-1">
      <div class="kh-head">
        <span class="kh-title">Keyboard</span>
        <span class="kh-hint">esc to close</span>
      </div>
      <div class="kh-grid">
        {#each groups as g (g.title)}
          <div class="kh-group">
            <div class="kh-grouphead">{g.title}</div>
            {#each g.rows as [combo, label] (combo)}
              <div class="kh-row">
                <kbd class="kh-keys">{combo}</kbd>
                <span class="kh-label">{label}</span>
              </div>
            {/each}
          </div>
        {/each}
      </div>
    </div>
  </div>
{/if}

<style>
  .kh-modal {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .kh-backdrop {
    position: absolute;
    inset: 0;
    border: 0;
    padding: 0;
    background: color-mix(in srgb, var(--bg) 62%, transparent);
    backdrop-filter: blur(3px);
    cursor: default;
  }
  .kh-card {
    position: relative;
    width: min(540px, calc(100vw - 48px));
    background: var(--elev, var(--panel));
    border: 1px solid var(--edge);
    border-radius: 12px;
    box-shadow: 0 18px 48px -18px rgba(0, 0, 0, 0.55);
    padding: 16px 18px 18px;
  }
  .kh-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    margin-bottom: 14px;
  }
  .kh-title {
    font-size: 13px;
    font-weight: 600;
    letter-spacing: -0.01em;
    color: var(--ink);
  }
  .kh-hint {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink-faint);
  }
  .kh-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 18px;
  }
  .kh-grouphead {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ink-faint);
    margin-bottom: 8px;
  }
  .kh-row {
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 3px 0;
  }
  .kh-keys {
    flex: 0 0 76px;
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--accent);
    letter-spacing: 0.02em;
    white-space: nowrap;
  }
  .kh-label {
    font-size: 12px;
    color: var(--ink-dim);
  }
</style>
