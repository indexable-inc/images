<script lang="ts">
  import type { Pane } from '$lib/types';

  // The html renderer: the pane's `body` is a self-contained HTML document the
  // producer ships. It mounts in a sandboxed frame so a producer can define its
  // own UI without the dashboard learning the resource. `allow-scripts` without
  // `allow-same-origin` keeps it interactive but in an opaque origin: it cannot
  // reach the parent page, its cookies, or its storage.
  let { pane }: { pane: Pane } = $props();

  // Opt the frame document into both color schemes so it tracks the OS theme like
  // the rest of the dashboard (which themes via `light-dark()`). Without this a
  // sandboxed srcdoc frame defaults its canvas to light — WebKit/Safari renders a
  // jarring white box behind otherwise-dark pane content (e.g. polars tables) on a
  // dark page. The `<meta>` lands in the document head whether the producer ships
  // a fragment (parser implies head/body) or a full document with its own <head>;
  // a producer that sets its own color-scheme still wins (its tag parses later).
  const SCHEME = '<meta name="color-scheme" content="light dark">';
  function themed(body: string): string {
    const head = body.match(/<head[^>]*>/i);
    if (head) {
      const at = head.index! + head[0].length;
      return body.slice(0, at) + SCHEME + body.slice(at);
    }
    return SCHEME + body;
  }
  const html = $derived(themed(pane.body ?? ''));
</script>

<iframe
  class="html-frame"
  title={pane.title || 'html pane'}
  sandbox="allow-scripts"
  srcdoc={html}
></iframe>
