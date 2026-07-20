<script lang="ts">
  import Terminal from '$components/Terminal.svelte';
  import type { TermConnection } from '$lib/term.svelte';

  const { conn, name }: { conn: TermConnection; name: string } = $props();

  const docSrc = $derived(
    conn.doc === null
      ? null
      : `/api/sessions/${encodeURIComponent(conn.sessionId)}/doc?nonce=${String(conn.doc.nonce)}`
  );
</script>

<div class="session">
  <header class="bar">
    <span class="title">{name}</span>
    <span class="chip" class:driving={conn.isDriver}>
      {conn.isDriver ? 'driving' : 'viewing'}
    </span>
    {#if !conn.connected}
      <span class="chip offline">reconnecting...</span>
    {/if}
  </header>

  {#if conn.openError !== null}
    <div class="banner error">
      <span>{conn.openError}</span>
      <button
        onclick={() => {
          conn.dismissError();
        }}>dismiss</button
      >
    </div>
  {/if}

  {#if conn.exit !== null}
    <div class="banner exit">
      process exited{conn.exit.code === null
        ? ''
        : ` (code ${String(conn.exit.code)})`}
    </div>
  {/if}

  <div class="panes">
    <div class="term-pane">
      <Terminal {conn} />
    </div>
    {#if conn.doc !== null && docSrc !== null}
      <div class="doc-pane">
        <div class="doc-header">
          <span class="doc-path" title={conn.doc.path}>{conn.doc.path}</span>
          <button
            aria-label="close document"
            onclick={() => {
              conn.closeDoc();
            }}>x</button
          >
        </div>
        <iframe sandbox="allow-scripts" src={docSrc} title={conn.doc.path}
        ></iframe>
      </div>
    {/if}
  </div>
</div>

<style>
  .session {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    border-bottom: 1px solid #262626;
    background: #171717;
  }

  .title {
    color: #ccc;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chip {
    font-size: 11px;
    padding: 1px 8px;
    border-radius: 9px;
    background: #262626;
    color: #999;
  }

  .chip.driving {
    background: #1d3324;
    color: #7fc98b;
  }

  .chip.offline {
    background: #3a2a1a;
    color: #d9a05b;
  }

  .banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 5px 10px;
    font-size: 12px;
  }

  .banner.error {
    background: #3a1d1d;
    color: #e39a9a;
  }

  .banner.error button {
    background: none;
    border: 1px solid #5a3333;
    border-radius: 3px;
    color: #e39a9a;
    cursor: pointer;
    padding: 1px 8px;
  }

  .banner.exit {
    background: #26262e;
    color: #a9a9c9;
  }

  .panes {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  .term-pane {
    flex: 1;
    min-width: 0;
    display: flex;
  }

  .doc-pane {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    border-left: 1px solid #262626;
    background: #141414;
  }

  .doc-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 8px;
    border-bottom: 1px solid #262626;
    font-size: 12px;
    color: #999;
  }

  .doc-path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .doc-header button {
    background: none;
    border: none;
    color: #777;
    cursor: pointer;
    padding: 0 4px;
  }

  .doc-header button:hover {
    color: #ddd;
  }

  iframe {
    flex: 1;
    border: none;
    background: #fff;
  }
</style>
