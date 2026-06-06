<script lang="ts">
  import * as api from '$lib/api';
  import { parseDiffFiles } from '$lib/diff';
  import type { ThreadPanelTab } from '$lib/threadPanels';
  import DiffBlock from './DiffBlock.svelte';
  import IconFile from '~icons/ph/file-text';
  import IconFolder from '~icons/ph/folder-open';
  import IconGitDiff from '~icons/ph/git-diff';

  interface Props {
    serverId: string;
    threadId: string;
    tab: ThreadPanelTab;
    onTab: (tab: ThreadPanelTab) => void;
  }

  let { serverId, threadId, tab, onTab }: Props = $props();

  let changed = $state<api.ChangedFile[]>([]);
  let selectedPath = $state<string | null>(null);
  let diff = $state('');
  let reviewError = $state<string | null>(null);
  let files = $state<api.FileListing | null>(null);
  let openedFile = $state<api.FileContents | null>(null);
  let fileError = $state<string | null>(null);
  let loadingReview = $state(false);
  let loadingFiles = $state(false);
  let loadingFile = $state(false);
  let panel = $state<HTMLElement | null>(null);
  let panelWidth = $state(520);
  let reviewListPct = $state(34);

  $effect(() => {
    const id = threadId;
    loadingReview = true;
    reviewError = null;
    api
      .listChangedFiles(serverId, id)
      .then((files) => {
        if (id !== threadId) return;
        changed = files;
        selectedPath = files[0]?.path ?? null;
      })
      .catch((err) => {
        if (id === threadId) reviewError = (err as Error).message;
      })
      .finally(() => {
        if (id === threadId) loadingReview = false;
      });
  });

  $effect(() => {
    const id = threadId;
    const path = selectedPath;
    if (tab !== 'review') return;
    api
      .getThreadDiff(serverId, id, path)
      .then((value) => {
        if (id === threadId && path === selectedPath) diff = value;
      })
      .catch((err) => {
        if (id === threadId && path === selectedPath) reviewError = (err as Error).message;
      });
  });

  function openDir(path: string | null) {
    const id = threadId;
    loadingFiles = true;
    fileError = null;
    openedFile = null;
    api
      .listThreadFiles(serverId, id, path)
      .then((listing) => {
        if (id === threadId) files = listing;
      })
      .catch((err) => {
        if (id === threadId) fileError = (err as Error).message;
      })
      .finally(() => {
        if (id === threadId) loadingFiles = false;
      });
  }

  function openFile(path: string) {
    const id = threadId;
    loadingFile = true;
    fileError = null;
    api
      .readThreadFile(serverId, id, path)
      .then((file) => {
        if (id === threadId) openedFile = file;
      })
      .catch((err) => {
        if (id === threadId) fileError = (err as Error).message;
      })
      .finally(() => {
        if (id === threadId) loadingFile = false;
      });
  }

  $effect(() => {
    if (tab === 'files' && !files && !loadingFiles) openDir(null);
  });

  function parentPath(path: string): string | null {
    if (!path) return null;
    const idx = path.lastIndexOf('/');
    return idx <= 0 ? '' : path.slice(0, idx);
  }

  function count(value: number | null): string {
    return value == null ? '-' : String(value);
  }

  function onPanelResizeDown(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = panelWidth;
    const onMove = (m: PointerEvent) => {
      panelWidth = Math.min(900, Math.max(340, startWidth + startX - m.clientX));
    };
    const onUp = () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp, { once: true });
  }

  function onReviewSplitDown(e: PointerEvent) {
    if (e.button !== 0 || !panel) return;
    e.preventDefault();
    const review = panel.querySelector('.review');
    if (!(review instanceof HTMLElement)) return;
    const rect = review.getBoundingClientRect();
    const onMove = (m: PointerEvent) => {
      const pct = ((m.clientY - rect.top) / Math.max(1, rect.height)) * 100;
      reviewListPct = Math.min(70, Math.max(18, pct));
    };
    const onUp = () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp, { once: true });
  }

  let diffFiles = $derived(parseDiffFiles(diff));
</script>

<aside
  class="side-panel"
  aria-label="Thread side panel"
  bind:this={panel}
  style:width={panelWidth + 'px'}
>
  <button
    class="panel-resizer"
    type="button"
    aria-label="Resize side panel"
    onpointerdown={onPanelResizeDown}
  ></button>
  <div class="tabs">
    <button class:active={tab === 'review'} type="button" onclick={() => onTab('review')}>
      <IconGitDiff width={14} height={14} />
      <span>Review</span>
      {#if changed.length > 0}<strong>{changed.length}</strong>{/if}
    </button>
    <button class:active={tab === 'files'} type="button" onclick={() => onTab('files')}>
      <IconFolder width={14} height={14} />
      <span>Files</span>
    </button>
  </div>

  {#if tab === 'review'}
    <section class="review">
      {#if reviewError}
        <p class="state error">{reviewError}</p>
      {:else if loadingReview}
        <p class="state">Loading changes...</p>
      {:else if changed.length === 0}
        <p class="state">No changes in this session.</p>
      {:else}
        <div class="changed-list" style:flex-basis={reviewListPct + '%'}>
          {#each changed as file (file.path)}
            <button
              type="button"
              class:selected={file.path === selectedPath}
              onclick={() => (selectedPath = file.path)}
              title={file.path}
            >
              <span class="status">{file.status}</span>
              <span class="path">{file.path}</span>
              <span class="stat add">+{count(file.additions)}</span>
              <span class="stat del">-{count(file.deletions)}</span>
            </button>
          {/each}
        </div>
        <button
          class="split-resizer"
          type="button"
          aria-label="Resize review sections"
          onpointerdown={onReviewSplitDown}
        ></button>
        <div class="diff-pane">
          {#if diffFiles.length === 0}
            <p class="state">No unified diff for this file.</p>
          {:else}
            {#each diffFiles as file, i (file.name + i)}
              <DiffBlock {file} />
            {/each}
          {/if}
        </div>
      {/if}
    </section>
  {:else}
    <section class="files">
      {#if fileError}
        <p class="state error">{fileError}</p>
      {:else if loadingFiles && !files}
        <p class="state">Loading files...</p>
      {:else if files}
        <div class="file-head">
          <button type="button" disabled={!files.path} onclick={() => openDir(parentPath(files?.path ?? ''))}>
            ..
          </button>
          <code>{files.path || '/'}</code>
        </div>
        <div class="file-browser">
          <div class="file-list">
            {#each files.entries as entry (entry.path)}
              <button
                type="button"
                class:file={!entry.is_dir}
                class:selected={openedFile?.path === entry.path}
                onclick={() => (entry.is_dir ? openDir(entry.path) : openFile(entry.path))}
                title={entry.path}
              >
                {#if entry.is_dir}
                  <IconFolder width={14} height={14} />
                {:else}
                  <IconFile width={14} height={14} />
                {/if}
                <span>{entry.name}</span>
              </button>
            {/each}
          </div>
          <div class="file-view">
            {#if loadingFile}
              <p class="state">Loading file...</p>
            {:else if openedFile}
              <div class="file-view-head">
                <IconFile width={14} height={14} />
                <code>{openedFile.path}</code>
                {#if openedFile.truncated}<span>truncated</span>{/if}
              </div>
              <pre>{openedFile.contents}</pre>
            {:else}
              <p class="state">Select a file to preview it.</p>
            {/if}
          </div>
        </div>
      {/if}
    </section>
  {/if}
</aside>

<style>
  /* The panel sits inside the workbench row (between chat header
     and composer) and stretches to that row's height. Internal
     content scrolls; the panel itself never grows past its
     container, so the diff viewer scrolls inside a fixed frame. */
  .side-panel {
    min-width: 340px;
    max-width: min(70vw, 900px);
    min-height: 0;
    border-left: 1px solid var(--border);
    background: var(--bg-pane);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    position: relative;
    flex-shrink: 0;
  }
  /* Drag handle for the entire panel width. Wider hit target than
     it appears so users can grab it without precision; a thin
     accent line shows up on hover/active so it's discoverable. */
  .panel-resizer {
    position: absolute;
    top: 0;
    bottom: 0;
    left: -4px;
    z-index: 5;
    width: 8px;
    cursor: col-resize;
    background: transparent;
    border: 0;
    padding: 0;
  }
  .panel-resizer::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 50%;
    width: 1px;
    transform: translateX(-0.5px);
    background: transparent;
    transition: background 0.12s ease;
  }
  .panel-resizer:hover::after,
  .panel-resizer:active::after {
    background: var(--text-dim);
  }
  .tabs {
    height: 42px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 7px 10px 7px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
  }
  .tabs button {
    height: 26px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 9px;
    border-radius: 7px;
    color: var(--text-muted);
    font-size: 12px;
  }
  .tabs button.active {
    background: var(--bg-active);
    color: var(--text-strong);
  }
  .tabs strong {
    min-width: 16px;
    height: 16px;
    border-radius: 999px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: var(--bg-pill-hi);
    color: var(--text-muted);
    font-size: 10px;
  }
  .review,
  .files {
    min-height: 0;
    display: flex;
    flex: 1;
    flex-direction: column;
    overflow: hidden;
  }
  .changed-list {
    flex-shrink: 0;
    overflow: auto;
    padding: 7px 10px 6px;
    min-height: 80px;
  }
  /* Vertical splitter between the changed-files list and the diff
     viewport. Same visibility model as the panel resizer: hit
     target is taller than the indicator so it's easy to grab. */
  .split-resizer {
    height: 8px;
    flex-shrink: 0;
    border: 0;
    background: transparent;
    cursor: row-resize;
    padding: 0;
    position: relative;
  }
  .split-resizer::before {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    top: 50%;
    height: 1px;
    transform: translateY(-0.5px);
    background: var(--border);
    transition: background 0.12s ease;
  }
  .split-resizer:hover::before,
  .split-resizer:active::before {
    background: var(--text-dim);
  }
  .changed-list button,
  .file-list button {
    width: 100%;
    min-width: 0;
    display: grid;
    align-items: center;
    gap: 7px;
    border-radius: 6px;
    color: var(--text-muted);
    text-align: left;
  }
  .changed-list button {
    grid-template-columns: 28px 1fr auto auto;
    padding: 6px 8px;
  }
  .changed-list button.selected,
  .changed-list button:hover {
    background: var(--bg-active);
    color: var(--text-strong);
  }
  .status,
  .stat,
  .file-head code {
    font-family: var(--font-mono);
    font-size: 11px;
  }
  .path,
  .file-list span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .add { color: rgba(76, 217, 100, 0.9); }
  .del { color: rgba(255, 105, 97, 0.9); }
  .diff-pane {
    flex: 1;
    min-height: 0;
    min-width: 0;
    overflow: hidden;
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .diff-pane :global(.diff) {
    flex: 1;
    min-height: 0;
  }
  .file-head {
    height: 34px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 10px;
    border-bottom: 1px solid var(--border);
    color: var(--text-dim);
  }
  .file-head button {
    width: 24px;
    height: 22px;
    border-radius: 5px;
    background: var(--bg-pill);
    color: var(--text-muted);
  }
  .file-head button:disabled {
    opacity: 0.35;
  }
  .file-list {
    overflow: auto;
    padding: 6px;
  }
  .file-list button {
    grid-template-columns: 18px 1fr;
    padding: 7px 8px;
  }
  .file-list button:hover,
  .file-list button.selected {
    background: var(--bg-active);
    color: var(--text-strong);
  }
  .file-list button.file {
    cursor: pointer;
  }
  .file-browser {
    display: grid;
    grid-template-rows: minmax(120px, 34%) 1fr;
    min-height: 0;
    flex: 1;
  }
  .file-view {
    min-height: 0;
    border-top: 1px solid var(--border);
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
  .file-view-head {
    height: 34px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    border-bottom: 1px solid var(--border);
    color: var(--text-muted);
    min-width: 0;
  }
  .file-view-head code {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-strong);
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .file-view-head span {
    margin-left: auto;
    color: var(--text-dim);
    font-size: 11px;
  }
  .file-view pre {
    margin: 0;
    flex: 1;
    overflow: auto;
    padding: 12px 14px 24px;
    color: var(--text);
    background: var(--bg-elev);
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.55;
    white-space: pre;
  }
  .state {
    margin: 0;
    padding: 18px 14px;
    color: var(--text-dim);
    font-size: 12px;
  }
  .state.error {
    color: var(--danger);
  }
</style>
