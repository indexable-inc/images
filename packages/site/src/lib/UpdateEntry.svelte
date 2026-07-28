<script lang="ts">
  import { formatPostedAt } from './format-posted-at';
  import { inlineTitleHtml, type SiteUpdate } from './updates';

  type Props = {
    update: SiteUpdate;
    timeZone: string | undefined;
    // `h1` for standalone permalink pages, `h2` (default) on the feed.
    titleTag?: 'h1' | 'h2';
    // Root under which entry permalinks live, with its trailing slash. A
    // host app passes its own resolved base (the index app passes
    // `resolve('/')`); the default suits root-mounted consumers. Kept a
    // plain string so the library never names a host route id.
    permalinkBase?: string;
  };

  const { update, timeZone, titleTag = 'h2', permalinkBase = '/' }: Props = $props();

  const Body = $derived(update.component);
  const titleHtml = $derived(inlineTitleHtml(update.title));
  const label = $derived(formatPostedAt(update.postedAt, timeZone));
  const permalink = $derived(`${permalinkBase}${update.id}`);
</script>

<article id={update.id}>
  <time datetime={update.postedAt}>{label}</time>
  {#if titleTag === 'h1'}
    <h1>
      <!-- eslint-disable-next-line svelte/no-at-html-tags -->
      {@html titleHtml}
    </h1>
  {:else}
    <h2>
      <!-- The permalink joins the caller's resolve()d base with the entry id;
           a library component cannot name a host route id in resolve(). -->
      <!-- eslint-disable-next-line svelte/no-navigation-without-resolve, svelte/no-at-html-tags -->
      <a href={permalink}>{@html titleHtml}</a>
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
  {#if update.tags.length > 0}
    <ul class="tags" aria-label="Tags">
      {#each update.tags as tag (tag)}
        <li>{tag}</li>
      {/each}
    </ul>
  {/if}
</article>
