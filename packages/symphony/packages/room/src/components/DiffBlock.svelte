<script lang="ts">
  // Render one parsed file using Pierre's full diff renderer.
  //
  // Wire-up:
  //   1. `parseDiffFiles` (lib/diff) used Pierre's `parsePatchFiles`
  //      to turn the raw unified diff into per-file metadata.
  //   2. `preloadFileDiff` (Pierre's SSR helper) takes one
  //      FileDiffMetadata and returns a `prerenderedHTML` string
  //      that contains the styled diff: line numbers, gutter,
  //      additions/deletions, hunk separators, syntax-highlighted
  //      tokens via Shiki.
  //   3. We mount that HTML inside a `<diffs-container>` custom
  //      element so Pierre's stylesheet (shipped via the
  //      web-components side-effect import) attaches to its shadow
  //      root. This is the standard hydration entry point per the
  //      `@pierre/diffs/dist/components/web-components.js` source.
  //
  // The result is the real Pierre layout, not just Shiki's `diff`
  // grammar over plain text.

  import { preloadFileDiff } from '@pierre/diffs/ssr';
  import type { DiffFile } from '$lib/diff';

  const DIFFS_TAG = 'diffs-container';

  if (typeof customElements !== 'undefined' && !customElements.get(DIFFS_TAG)) {
    customElements.define(
      DIFFS_TAG,
      class extends HTMLElement {
        constructor() {
          super();
          if (!this.shadowRoot) this.attachShadow({ mode: 'open' });
        }
      }
    );
  }

  interface Props {
    file: DiffFile;
  }

  let { file }: Props = $props();

  let host = $state<HTMLElement | null>(null);
  let error = $state<string | null>(null);
  let renderToken = 0;

  $effect(() => {
    if (!host) return;
    const token = ++renderToken;
    error = null;
    void (async () => {
      try {
        // Renderer options. The two important ones here:
        //
        // * `overflow: 'wrap'` keeps long lines inside the panel
        //   width. The default 'scroll' mode horizontally scrolls
        //   the code while the gutter stays sticky, which produces
        //   the "lines start mid-word" rendering we kept seeing
        //   when the side panel was narrow. Wrap mode hard-wraps
        //   instead, which reads correctly at every width.
        //
        // * `diffIndicators: 'none'` drops the +/- column. We still
        //   get add/delete background tinting from the row classes,
        //   so the change type is visually clear without the
        //   leading character noise.
        //
        // Both are documented at
        // https://www.npmjs.com/package/@pierre/diffs (see
        // `BaseDiffOptions` in the package's `types.d.ts`).
        const result = await preloadFileDiff({
          fileDiff: file,
          options: {
            theme: 'pierre-dark-soft',
            diffStyle: 'unified',
            diffIndicators: 'none',
            overflow: 'wrap',
            stickyHeader: false,
            disableFileHeader: true
          }
        });
        if (token === renderToken && host?.shadowRoot) {
          host.shadowRoot.innerHTML = result.prerenderedHTML;
        }
      } catch (err) {
        if (token === renderToken) error = (err as Error)?.message ?? 'diff render failed';
      }
    })();
  });
</script>

<div class="diff">
  <div class="diff-head">
    <span class="diff-icon">
      <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor"
        stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
        <path d="M14 2v6h6" />
        <path d="M9 14h6" />
        <path d="M12 11v6" />
      </svg>
    </span>
    <span class="diff-path">{file.name}</span>
  </div>

  <diffs-container class="diff-body" bind:this={host}></diffs-container>

  {#if error}
    <pre class="diff-fallback">{error}</pre>
  {/if}
</div>

<style>
  .diff {
    margin: 0;
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg-elev);
    min-height: 0;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .diff-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pill);
    color: var(--text-muted);
    font-size: 12px;
    flex-shrink: 0;
    min-width: 0;
  }
  .diff-icon {
    color: var(--text-dim);
    display: inline-flex;
    flex-shrink: 0;
  }
  .diff-path {
    color: var(--text-strong);
    font-family: var(--font-mono);
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .diff-body {
    display: block;
    flex: 1;
    min-height: 0;
    min-width: 0;
    font-family: var(--font-mono);
    font-size: 12.5px;
    line-height: 1.55;
    overflow: auto;
    background: var(--bg-elev);
  }
  .diff-fallback {
    margin: 0;
    padding: 12px 14px;
    font-family: var(--font-mono);
    font-size: 12.5px;
    color: var(--danger);
    white-space: pre;
    overflow-x: auto;
  }
</style>
