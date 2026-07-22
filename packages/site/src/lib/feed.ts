import {
  plainText,
  siteIntro,
  siteUpdates,
  siteUrl,
  updateScript,
  updateUrl,
  type SiteUpdate
} from './updates';

// RSS 2.0 builder for the updates feed. Lives in the library (not the
// feed.xml route) so ix.dev renders the same feed from the same
// implementation; the defaults reproduce the live ix.dev output, and a
// consumer serving the content elsewhere overrides `siteUrl` to emit its
// own permalinks.
export type FeedOptions = {
  // Absolute site root with a trailing slash; entry permalinks append ids.
  siteUrl?: string;
  title?: string;
  description?: string;
  updates?: SiteUpdate[];
};

export const feedContentType = 'application/rss+xml; charset=utf-8';

export function buildFeedXml(options: FeedOptions = {}): string {
  const base = options.siteUrl ?? siteUrl;
  const title = options.title ?? 'index';
  const description = options.description ?? siteIntro;
  const updates = options.updates ?? siteUpdates;
  const feedUrl = `${base}feed.xml`;

  // An empty feed is still a valid channel; date it at generation time.
  const lastBuildDate = rssDate(updates[0]?.postedAt ?? new Date().toISOString());
  const items = updates.map((update) => itemXml(update, base)).join('');

  return `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom">
  <channel>
    <title>${escapeXml(title)}</title>
    <link>${escapeXml(base)}</link>
    <atom:link href="${escapeXml(feedUrl)}" rel="self" type="application/rss+xml" />
    <description>${escapeXml(description)}</description>
    <language>en-us</language>
    <lastBuildDate>${escapeXml(lastBuildDate)}</lastBuildDate>${items}
  </channel>
</rss>
`;
}

function itemXml(update: SiteUpdate, base: string): string {
  const link = updateUrl(update.id, base);
  const body = updateScript(update);

  return `
    <item>
      <title>${escapeXml(plainText(update.title))}</title>
      <link>${escapeXml(link)}</link>
      <guid isPermaLink="true">${escapeXml(link)}</guid>
      <pubDate>${escapeXml(rssDate(update.postedAt))}</pubDate>
      <description>${escapeXml(body)}</description>
    </item>`;
}

function rssDate(postedAt: string): string {
  return new Date(postedAt).toUTCString();
}

function escapeXml(value: string): string {
  return value.replace(/[<>&'"]/g, (character) => {
    switch (character) {
      case '<':
        return '&lt;';
      case '>':
        return '&gt;';
      case '&':
        return '&amp;';
      case "'":
        return '&apos;';
      case '"':
        return '&quot;';
      default:
        return character;
    }
  });
}
