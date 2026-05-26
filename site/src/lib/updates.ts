import type { Component } from 'svelte';

export type SiteUpdateLink = {
  label: string;
  href: string;
};

export type SiteUpdateMeta = {
  id: string;
  // ISO 8601 with timezone offset. Authors keep the source readable in their
  // local zone; render layers normalize to UTC so visitors in any zone see
  // one canonical date and time.
  postedAt: string;
  // Markdown source: backticks for inline code, asterisks for emphasis.
  title: string;
  links: SiteUpdateLink[];
  // Lowercased tag slugs. `interesting` is the default front-page filter; the
  // rest are free-form axes consumed by the boolean filter expression.
  tags: string[];
};

export type SiteUpdate = SiteUpdateMeta & {
  component: Component;
  rawBody: string;
};

type SvxModule = {
  default: Component;
  // The raw frontmatter shape. `tags` is optional here because mdsvex does
  // not validate; the loader below normalizes to a required `string[]`.
  metadata: Omit<SiteUpdateMeta, 'tags'> & { tags?: string[] };
};

const modules = import.meta.glob<SvxModule>('./updates/*.svx', { eager: true });
const rawModules = import.meta.glob<string>('./updates/*.svx', {
  eager: true,
  query: '?raw',
  import: 'default'
});

export const siteUpdates: SiteUpdate[] = Object.entries(modules)
  .map(([path, mod]) => ({
    ...mod.metadata,
    tags: (mod.metadata.tags ?? []).map((tag) => tag.toLowerCase()),
    component: mod.default,
    rawBody: stripFrontmatter(rawModules[path] ?? '')
  }))
  .sort((a, b) => Date.parse(b.postedAt) - Date.parse(a.postedAt));

export const siteUrl = 'https://indexable-inc.github.io/index/';
export const siteFeedUrl = `${siteUrl}feed.xml`;
export const siteIntro =
  'Images, NixOS modules, helpers, and notes published by ix.';

function stripFrontmatter(source: string): string {
  return source.replace(/^---\n[\s\S]*?\n---\n?/, '').trim();
}

export function plainText(markdown: string): string {
  return markdown
    .replace(/<script[\s\S]*?<\/script>/gi, '')
    .replace(/<[A-Z][A-Za-z0-9]*\b[^>]*\/?>/g, '')
    .replace(/<\/[A-Z][A-Za-z0-9]*>/g, '')
    .replace(/```[\s\S]*?```/g, '')
    .replace(/`([^`]+)`/g, '$1')
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/\*([^*]+)\*/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
    .replace(/\s+/g, ' ')
    .trim();
}

export function inlineTitleHtml(markdown: string): string {
  // Titles use markdown-style backticks for inline code. Escape HTML first,
  // then unescape the backtick run we matched so the <code> wrapper survives.
  return escapeHtml(markdown).replace(
    /`([^`]+)`/g,
    (_, code: string) => `<code>${code}</code>`
  );
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

export function updateScript(update: SiteUpdate): string {
  return `${plainText(update.title)}. ${plainText(update.rawBody)}`;
}

// Absolute URL for one entry. The base path lives in `siteUrl`, which
// already carries the trailing slash, so the slug just appends.
export function updateUrl(id: string): string {
  return `${siteUrl}${id}`;
}
