import type { Component } from 'svelte';

export type RfcMeta = {
  id: string;
  number: string;
  // Markdown source: backticks for inline code.
  title: string;
  status: string;
  authors: string;
  // YYYY-MM-DD.
  created: string;
  updated: string;
  trackingIssue: string | null;
  supersedes: string | null;
  supersededBy: string | null;
  // Markdown source. Shown under the title where present.
  description?: string;
  // 0000-template: excluded from the index, still reachable by id so
  // "copy the template" has something to link to.
  template?: boolean;
};

export type Rfc = RfcMeta & {
  component: Component;
};

type SvxModule = {
  default: Component;
  metadata: RfcMeta;
};

const modules = import.meta.glob<SvxModule>('./rfcs/*.svx', { eager: true });

const allRfcs: Rfc[] = Object.values(modules)
  .map((mod) => ({
    ...mod.metadata,
    component: mod.default
  }))
  .sort((a, b) => a.number.localeCompare(b.number));

// Listing order: by RFC number, template excluded.
export const rfcs: Rfc[] = allRfcs.filter((rfc) => !rfc.template);

export function findRfc(id: string): Rfc | undefined {
  return allRfcs.find((rfc) => rfc.id === id);
}

export function rfcEntries(): { id: string }[] {
  return allRfcs.map((rfc) => ({ id: rfc.id }));
}
