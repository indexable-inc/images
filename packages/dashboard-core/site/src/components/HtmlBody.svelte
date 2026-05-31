<script lang="ts">
  import type { Pane } from '$lib/types';

  // The html renderer: the pane's `body` is a self-contained HTML document the
  // producer ships. It mounts in a sandboxed frame so a producer can define its
  // own UI without the dashboard learning the resource. `allow-scripts` without
  // `allow-same-origin` keeps it interactive but in an opaque origin: it cannot
  // reach the parent page, its cookies, or its storage.
  let { pane }: { pane: Pane } = $props();
  const html = $derived(pane.body ?? '');
</script>

<iframe
  class="html-frame"
  title={pane.title || 'html pane'}
  sandbox="allow-scripts"
  srcdoc={html}
></iframe>
