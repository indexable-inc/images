<script lang="ts">
  import { inlineTitleHtml } from './updates';
  import type { Rfc } from './rfcs';

  const { rfc }: { rfc: Rfc } = $props();

  const Body = $derived(rfc.component);
  const titleHtml = $derived(inlineTitleHtml(`RFC ${rfc.number}: ${rfc.title}`));
  const descriptionHtml = $derived(rfc.description ? inlineTitleHtml(rfc.description) : undefined);
</script>

<article id={rfc.id}>
  <h1>
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    {@html titleHtml}
  </h1>
  {#if descriptionHtml}
    <p class="subtitle">
      <!-- eslint-disable-next-line svelte/no-at-html-tags -->
      {@html descriptionHtml}
    </p>
  {/if}
  <dl class="frontmatter">
    <dt>Status</dt><dd>{rfc.status}</dd>
    <dt>Authors</dt><dd>{rfc.authors}</dd>
    <dt>Created</dt><dd>{rfc.created}</dd>
    <dt>Updated</dt><dd>{rfc.updated}</dd>
    <dt>Tracking issue</dt><dd>{rfc.trackingIssue ?? '—'}</dd>
    <dt>Supersedes</dt><dd>{rfc.supersedes ?? '—'}</dd>
    <dt>Superseded by</dt><dd>{rfc.supersededBy ?? '—'}</dd>
  </dl>
  <div class="body">
    <Body />
  </div>
</article>
