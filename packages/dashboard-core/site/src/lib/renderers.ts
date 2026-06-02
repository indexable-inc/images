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
import HtmlBody from '$components/HtmlBody.svelte';
import TermBody from '$components/TermBody.svelte';
import type { Pane } from './types';

export const renderers: Record<string, Component<{ pane: Pane }>> = {
  terminal: TermBody,
  html: HtmlBody,
  exec: ExecBody,
  data: DataBody,
};

export const fallback: Component<{ pane: Pane }> = DataBody;

export function rendererFor(kind: string | undefined): Component<{ pane: Pane }> {
  return (kind && renderers[kind]) || fallback;
}
