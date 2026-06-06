<script lang="ts">
  // Filter input that lives just under the top nav. The parent owns
  // the actual filtering — this is a controlled input that also fires
  // onSubmit when the user hits Enter (top match) and onCancel on Esc.

  interface Props {
    value: string;
    onChange: (next: string) => void;
    onSubmit: () => void;
    onCancel: () => void;
  }

  let { value, onChange, onSubmit, onCancel }: Props = $props();
</script>

<div class="search-bar">
  <input
    type="text"
    class="search-input"
    placeholder="Filter chats"
    value={value}
    oninput={(e) => onChange((e.currentTarget as HTMLInputElement).value)}
    spellcheck="false"
    onkeydown={(e) => {
      if (e.key === 'Escape') {
        onCancel();
        return;
      }
      if (e.key === 'Enter') {
        e.preventDefault();
        onSubmit();
      }
    }}
  />
</div>

<style>
  .search-bar {
    padding: 2px 12px 8px;
  }
  .search-input {
    width: 100%;
    background: var(--bg-pill);
    border: 1px solid var(--border-hi);
    border-radius: 7px;
    padding: 6px 10px;
    font-size: 13px;
    color: var(--text-strong);
    outline: none;
  }
  .search-input::placeholder {
    color: var(--text-dim);
  }
</style>
