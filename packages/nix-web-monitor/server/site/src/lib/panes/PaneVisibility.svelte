<script lang="ts">
  /// Context bridge between a pane slot and its content: tells the subtree
  /// whether the pane is currently shown. Needed because context is
  /// per-component while a group renders many panes -- each slot gets its own
  /// bridge, so `getPaneVisibility()` inside any pane's content answers for
  /// *that* pane. Lives inside the keyed content list; toggling `visible`
  /// only flips the prop and never remounts the content.

  import type { Snippet } from 'svelte';
  import { setPaneVisibility } from '$lib/panes/context';

  type Props = {
    visible: boolean;
    children: Snippet;
  };

  const { visible, children }: Props = $props();

  setPaneVisibility(() => visible);
</script>

{@render children()}
