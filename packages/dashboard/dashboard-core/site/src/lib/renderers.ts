// The renderer registry: a pane's `kind` maps to the component that draws its
// body. This is the one place the canvas learns about kinds; everything else is
// generic over panes. Add a kind by adding an entry here and (optionally) a
// matching `View` variant on the wire.
//
// A pane whose kind is absent falls back to the data/JSON view, so a producer
// can always show *something* without a frontend change — publish a `data` pane
// and it renders, register a renderer later for a native look.
import type { Component } from 'svelte';
import DataBody from '$components/DataBody.svelte';
import ExecBody from '$components/ExecBody.svelte';
import FileViewBody from '$components/FileViewBody.svelte';
import HtmlBody from '$components/HtmlBody.svelte';
import NamespaceBody from '$components/NamespaceBody.svelte';
import NixBuildBody from '$components/NixBuildBody.svelte';
import TermBody from '$components/TermBody.svelte';
import type { Pane } from './types';

export const renderers: Record<string, Component<{ pane: Pane }>> = {
  terminal: TermBody,
  html: HtmlBody,
  exec: ExecBody,
  data: DataBody,
};

// Named renderers for `data` panes. A producer publishes a `data` pane with a
// `renderer` name and the frontend routes to a matching component; an unknown
// name falls back to the generic JSON tree (DataBody). This is the generative-UI
// seam: a new structured view is one entry here plus its component — no wire or
// aggregator change. `namespace` (a Python session's live globals) is the first.
const dataRenderers: Record<string, Component<{ pane: Pane }>> = {
  namespace: NamespaceBody,
  // A read's highlighted file card (the kernel's `read` tool / `view.cat`).
  'file-view': FileViewBody,
  // A live Nix build tree (the kernel's `nix.run`/`nix.build`), streamed from the
  // nix-web-monitor emitter's BuildView.
  'nix-build': NixBuildBody,
};

export const fallback: Component<{ pane: Pane }> = DataBody;

export function rendererFor(
  kind: string | undefined,
  renderer?: string,
): Component<{ pane: Pane }> {
  if (kind === 'data' && renderer && dataRenderers[renderer]) return dataRenderers[renderer];
  return (kind && renderers[kind]) || fallback;
}
