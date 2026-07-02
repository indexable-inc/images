<script lang="ts">
  // The Ledger shell: a unified foldable sidebar drives a single center stage (a
  // run's detail, or a resource) with a collapsible namespace rail on the right and
  // an always-visible timeline scrubber in the bottom status bar. There are no
  // more top-level views — one selection, one stage.
  import { onMount } from 'svelte';
  import { store, timeline, connect } from '$lib/stream.svelte';
  import { ui, startClock } from '$lib/ui.svelte';
  import { installKeymap } from '$lib/keys.svelte';
  import { refreshRatio, bumpTheme } from '$lib/metrics.svelte';
  import { onThemeChange } from '$lib/ansi';
  import { seedDemo } from '$lib/demo';
  import { paneScope } from '$lib/scope';
  import { buildSidebar } from '$lib/sidebar';
  import { withKey, kindOf } from '$lib/run';
  import { rendererFor } from '$lib/renderers';
  import Sidebar from '$components/Sidebar.svelte';
  import RunDetail from '$components/RunDetail.svelte';
  import NamespaceRail from '$components/NamespaceRail.svelte';
  import Statusbar from '$components/Statusbar.svelte';
  import FocusView from '$components/FocusView.svelte';
  import KeyHelp from '$components/KeyHelp.svelte';
  import type { Pane } from '$lib/types';

  onMount(() => {
    refreshRatio();
    startClock();
    const teardownKeys = installKeymap();
    if (new URLSearchParams(location.search).has('demo')) {
      seedDemo();
    } else {
      connect();
    }
    if (document.fonts?.ready) document.fonts.ready.then(refreshRatio);
    const teardownTheme = onThemeChange(bumpTheme);
    return () => {
      teardownKeys();
      teardownTheme();
    };
  });

  const model = $derived(buildSidebar(store.panes, timeline.recordings));

  // Resolve the selection into the pane the center stage shows (a run or a
  // resource) plus its scope, so the rail can inspect the same session.
  const selected = $derived.by<{ pane: Pane; scope: string } | null>(() => {
    const sel = ui.selection;
    if (!sel || sel.kind === 'recording') return null;
    const rec = store.panes[sel.key];
    if (!rec) return null;
    const scope = paneScope(sel.key);
    return { pane: withKey(sel.key, rec, scope), scope };
  });

  // The label of the selected run's session, for the detail breadcrumb.
  const sessionLabel = $derived(
    selected ? (model.sessions.find((s) => s.scope === selected.scope)?.label ?? '') : '',
  );

  const isRunSelection = $derived(ui.selection?.kind === 'run');
</script>

<div class="shell">
  <div class="body">
    <Sidebar />

    <main class="stage">
      {#if selected}
        {#if isRunSelection}
          <RunDetail pane={selected.pane} {sessionLabel} />
        {:else}
          <!-- A resource fills the stage directly (terminal / browser). -->
          {@const Body = rendererFor(selected.pane.kind, selected.pane.renderer)}
          <div class="resource-stage pane" class:term={kindOf(selected.pane) === 'terminal'}>
            <div class="resource-head">
              <span class="resource-title">{selected.pane.title || '(resource)'}</span>
              {#if selected.pane.subtitle}<span class="resource-sub">{selected.pane.subtitle}</span>{/if}
            </div>
            <div class="body-scroll body" class:term-body={kindOf(selected.pane) === 'terminal'} class:html-body={kindOf(selected.pane) === 'html'}>
              <Body pane={selected.pane} />
            </div>
          </div>
        {/if}
      {:else if timeline.source !== 'live'}
        <div class="stage-empty">select a run or scrub the recording</div>
      {:else}
        <div class="stage-empty">{store.live ? 'select a run' : 'connecting…'}</div>
      {/if}

      <!-- The fullscreen single-pane overlay (o/Enter on a resource or rich output). -->
      {#if ui.focusKey}<FocusView />{/if}
    </main>

    {#if selected}<NamespaceRail scope={selected.scope} />{/if}
  </div>

  <Statusbar sessionCount={model.sessions.length} runCount={model.runCount} />
</div>

<!-- The keyboard cheatsheet (press ?), a global overlay above everything. -->
<KeyHelp />

<style>
  .shell {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .body {
    flex: 1 1 auto;
    display: flex;
    min-height: 0;
  }
  .stage {
    position: relative;
    flex: 1 1 auto;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .stage-empty {
    flex: 1 1 auto;
    display: grid;
    place-content: center;
    color: var(--ink-faint);
    font-family: var(--mono);
    font-size: 13px;
  }
  .resource-stage {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .resource-head {
    flex: none;
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 12px clamp(16px, 2.4vw, 24px);
    border-bottom: 1px solid var(--edge);
  }
  .resource-title {
    font-family: var(--mono);
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }
  .resource-sub {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--ink-faint);
  }
  .body-scroll {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
  }
  .resource-stage .body.html-body {
    height: 100%;
  }
</style>
