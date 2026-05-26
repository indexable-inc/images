import {
  plainText,
  siteFeedUrl,
  siteIntro,
  siteUpdates,
  siteUrl,
  updateScript
} from '$lib/updates';

export const prerender = true;

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

function rssDate(postedAt: string): string {
  return new Date(postedAt).toUTCString();
}

function updateUrl(updateId: string): string {
  return `${siteUrl}#${encodeURIComponent(updateId)}`;
}

function itemXml(update: (typeof siteUpdates)[number]): string {
  const link = updateUrl(update.id);
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

export function GET(): Response {
  const latestUpdate = siteUpdates[0];
  const lastBuildDate = rssDate(latestUpdate.postedAt);
  const items = siteUpdates.map(itemXml).join('');
  const xml = `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom">
  <channel>
    <title>index</title>
    <link>${escapeXml(siteUrl)}</link>
    <atom:link href="${escapeXml(siteFeedUrl)}" rel="self" type="application/rss+xml" />
    <description>${escapeXml(siteIntro)}</description>
    <language>en-us</language>
    <lastBuildDate>${escapeXml(lastBuildDate)}</lastBuildDate>${items}
  </channel>
</rss>
`;

  return new Response(xml, {
    headers: {
      'content-type': 'application/rss+xml; charset=utf-8'
    }
  });
}
