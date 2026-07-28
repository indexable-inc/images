<script lang="ts">
  let { isSample, onpick }: { isSample: boolean; onpick: (f: File) => void } = $props();
</script>

<header>
  <div class="brand">
    <span class="logo">◆</span>
    <h1>system&#8209;prompt&nbsp;evals</h1>
    {#if isSample}<span class="pill">sample</span>{/if}
  </div>
  <label class="loadbtn">
    load JSON
    <input
      type="file"
      accept="application/json,.json"
      onchange={(e) => {
        const f = (e.currentTarget as HTMLInputElement).files?.[0];
        if (f) onpick(f);
      }}
    />
  </label>
</header>

<style>
  header { display: flex; align-items: center; justify-content: space-between; margin-bottom: 14px; }
  .brand { display: flex; align-items: center; gap: 12px; }
  .logo { font-size: 18px; color: var(--accent); }
  h1 { font-size: 19px; font-weight: 600; letter-spacing: -0.01em; margin: 0; }
  .pill { font-size: 11px; color: var(--dim); background: var(--chip); border-radius: 4px; padding: 2px 8px; }
  .loadbtn { font-size: 13px; border: 1px solid var(--line-strong); border-radius: var(--radius); padding: 6px 13px;
    cursor: pointer; background: var(--card); transition: background 0.12s; }
  .loadbtn:hover { background: var(--panel); }
  .loadbtn input { display: none; }
</style>
