<script lang="ts">
  import type { Pane } from '$lib/types';

  // The html renderer: the pane's `body` is a self-contained HTML document the
  // producer ships. It mounts in a sandboxed frame so a producer can define its
  // own UI without the dashboard learning the resource. `allow-scripts` without
  // `allow-same-origin` keeps it interactive but in an opaque origin: it cannot
  // reach the parent page, its cookies, or its storage.
  let { pane }: { pane: Pane } = $props();

  // Opt the frame document into both color schemes so it tracks the OS theme like
  // the rest of the dashboard (style.css sets `color-scheme: light dark` on :root).
  // The <iframe> element inherits that scheme; if the frame *document* resolves to a
  // different one (an unstyled producer fragment defaults to `normal`/light), the
  // CSS Color Adjustment spec makes the browser paint an *opaque* canvas behind the
  // frame — the white box WebKit/Safari shows on a dark page. Declaring the same
  // `light dark` on the document makes the schemes match, so the canvas stays
  // transparent and the dashboard's dark surface shows through.
  //
  // We PREPEND a fixed prefix rather than parse: pure string concatenation never
  // inspects or mutates producer bytes, so there is no regex/DOM edge case to get
  // wrong. The `<meta>` only sets the document's default supported schemes; a
  // producer that sets its own `color-scheme` in author CSS still wins the used
  // value, so this only themes otherwise-unstyled producers (e.g. a polars table).
  // The leading `<!doctype html>` is the conformant srcdoc preamble; srcdoc
  // documents render no-quirks regardless of doctype, so it changes no layout mode.
  const PREFIX = '<!doctype html><meta name="color-scheme" content="light dark">';
  const html = $derived(PREFIX + (pane.body ?? ''));
</script>

<iframe
  class="html-frame"
  title={pane.title || 'html pane'}
  sandbox="allow-scripts"
  srcdoc={html}
></iframe>
