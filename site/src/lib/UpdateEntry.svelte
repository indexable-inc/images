<script lang="ts">
  import { resolve } from '$app/paths';
  import { formatPostedAt } from './format-posted-at';
  import { inlineTitleHtml, type SiteUpdate } from './updates';

  type Props = {
    update: SiteUpdate;
    timeZone: string | undefined;
    // `h1` for standalone permalink pages, `h2` (default) on the feed.
    titleTag?: 'h1' | 'h2';
    // The feed wants each title to link back to its permalink; the
    // permalink page itself does not need to link to itself.
    titleLinksToPermalink?: boolean;
  };

  const {
    update,
    timeZone,
    titleTag = 'h2',
    titleLinksToPermalink = true
  }: Props = $props();

  const Body = $derived(update.component);
  const titleHtml = $derived(inlineTitleHtml(update.title));
  const label = $derived(formatPostedAt(update.postedAt, timeZone));
  const permalink = $derived(resolve('/[id]', { id: update.id }));
</script>

<article id={update.id}>
  <time datetime={update.postedAt}>{label}</time>
  {#if titleTag === 'h1'}
    <h1>
      <!-- eslint-disable-next-line svelte/no-at-html-tags -->
      {@html titleHtml}
    </h1>
  {:else if titleLinksToPermalink}
    <h2>
      <!-- eslint-disable-next-line svelte/no-at-html-tags -->
      <a href={permalink}>{@html titleHtml}</a>
    </h2>
  {:else}
    <h2>
      <!-- eslint-disable-next-line svelte/no-at-html-tags -->
      {@html titleHtml}
    </h2>
  {/if}
  <div class="body">
    <Body />
  </div>
  {#if update.links.length > 0}
    <div class="refs">
      {#each update.links as link, i (link.href)}
        {#if i > 0}<span aria-hidden="true">·</span>{/if}
        <a href={link.href} rel="external">{link.label}</a>
      {/each}
    </div>
  {/if}
</article>
