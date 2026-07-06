// The sidebar model, derived once at module level from the live pane set plus
// the recordings list. `store.panes` is reassigned on every live SSE frame, so
// deriving this independently in each consumer (App's status bar, the Sidebar
// tree) rebuilt the whole model — pane scan plus sorts — once per consumer per
// frame; sharing one $derived rebuilds it a single time.
import { store, timeline } from './stream.svelte.ts';
import { buildSidebar, type SidebarModel } from './sidebar.ts';

const model = $derived(buildSidebar(store.panes, timeline.recordings));

// Svelte forbids exporting derived state from a module directly, so consumers
// read the shared instance through this accessor.
export function sidebarModel(): SidebarModel {
  return model;
}
