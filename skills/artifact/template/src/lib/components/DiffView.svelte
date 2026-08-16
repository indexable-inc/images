<script lang="ts">
  import { diffLines, foldContext } from '../diff';
  import type { Version } from '../versions';

  let { base, target }: { base: Version; target: Version } = $props();

  const hunks = $derived(foldContext(diffLines(base.source, target.source)));
  const stats = $derived.by(() => {
    let added = 0;
    let removed = 0;
    for (const hunk of hunks) {
      if (hunk.kind !== 'lines') continue;
      for (const line of hunk.lines) {
        if (line.kind === 'add') added++;
        if (line.kind === 'del') removed++;
      }
    }
    return { added, removed };
  });

  // The marker column carries add/delete without relying on color.
  const markers = { add: '+', del: '-', same: ' ' } as const;
</script>

<section>
  <p class="summary">
    {base.id} &rarr; {target.id}
    <span class="added">+{stats.added}</span>
    <span class="removed">&minus;{stats.removed}</span>
    {#if target.note}<span class="note">{target.note}</span>{/if}
  </p>

  <div class="diff">
    <div class="rows">
      {#each hunks as hunk, hunkIndex (hunkIndex)}
        {#if hunk.kind === 'skip'}
          <div class="row skip">
            <span class="gutter a"></span>
            <span class="gutter b"></span>
            <span class="marker"></span>
            <span class="text">&#8943; {hunk.count} unchanged lines</span>
          </div>
        {:else}
          {#each hunk.lines as line, lineIndex (lineIndex)}
            <div class="row {line.kind}">
              <span class="gutter a">{line.kind === 'add' ? '' : line.aLine}</span>
              <span class="gutter b">{line.kind === 'del' ? '' : line.bLine}</span>
              <span class="marker">{markers[line.kind]}</span>
              <span class="text">{line.text || ' '}</span>
            </div>
          {/each}
        {/if}
      {/each}
    </div>
  </div>
</section>

<style>
  .summary {
    font-size: 0.85rem;
    color: var(--fg-muted);
    margin: 0 0 0.75rem;
  }

  .added {
    color: var(--add-fg);
    font-weight: 600;
  }

  .removed {
    color: var(--del-fg);
    font-weight: 600;
  }

  .note {
    margin-left: 0.5rem;
  }

  /* Long lines scroll horizontally rather than being clipped mid-word;
     the number gutters and the +/- marker stay pinned while they do. */
  .diff {
    --gutter-w: 3rem;
    --marker-w: 1.25rem;
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow-x: auto;
    overscroll-behavior-x: contain;
    background: var(--bg);
    font-family: var(--font-mono);
    font-size: 0.8rem;
    line-height: 1.55;
    padding: 0.5rem 0;
  }

  /* One track sized to the longest line, so every row tint spans the
     whole scrollable width rather than stopping at the viewport edge;
     min-width keeps short diffs filling the panel. */
  .rows {
    width: max-content;
    min-width: 100%;
  }

  .row {
    display: grid;
    grid-template-columns:
      var(--gutter-w) var(--gutter-w)
      var(--marker-w) auto;
    white-space: pre;
  }

  /* No opacity here: it would fade the sticky backdrop too and let the
     code scroll through visibly behind the line numbers. */
  .gutter {
    color: var(--fg-muted);
    text-align: right;
    padding-right: 0.75rem;
    user-select: none;
    position: sticky;
    z-index: 1;
    background: var(--bg);
  }

  .gutter.a {
    left: 0;
  }

  .gutter.b {
    left: var(--gutter-w);
  }

  .marker {
    text-align: center;
    user-select: none;
    position: sticky;
    left: calc(var(--gutter-w) * 2);
    z-index: 1;
    background: var(--bg);
    color: var(--fg-muted);
  }

  .text {
    padding: 0 1rem 0 0.5rem;
  }

  .row.add {
    background: var(--add-bg);
  }

  /* Sticky cells need an opaque backdrop, so re-composite the row tint
     over the page background instead of letting text scroll under it. */
  .row.add .gutter,
  .row.add .marker {
    background:
      linear-gradient(var(--add-bg), var(--add-bg)),
      var(--bg);
  }

  .row.add .text,
  .row.add .marker {
    color: var(--add-fg);
  }

  .row.del {
    background: var(--del-bg);
  }

  .row.del .gutter,
  .row.del .marker {
    background:
      linear-gradient(var(--del-bg), var(--del-bg)),
      var(--bg);
  }

  .row.del .text,
  .row.del .marker {
    color: var(--del-fg);
  }

  .row.add .marker,
  .row.del .marker {
    font-weight: 700;
  }

  .row.skip .text {
    color: var(--fg-muted);
    padding-top: 0.35rem;
    padding-bottom: 0.35rem;
  }
</style>
