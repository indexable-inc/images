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

// RFC numbers derive from filenames (the digits before the first '-') and
// must be unique and agree with frontmatter. Throwing at module scope makes
// `vite build` — and so `nix build .#site` and CI — fail on a violation:
// the glob is eager and every prerendered RFC route imports this module.
// Two RFCs once merged as 0010 (#2154); this keeps that unrepresentable.
const numberToPath = new Map<string, string>();
for (const [path, mod] of Object.entries(modules)) {
  const stem = path.slice(path.lastIndexOf('/') + 1).replace(/\.svx$/, '');
  const number = stem.slice(0, stem.indexOf('-'));
  if (!/^\d{4}$/.test(number)) {
    throw new Error(`RFC ${path}: filename must start with a four-digit number and '-'`);
  }
  if (mod.metadata.number !== number) {
    throw new Error(
      `RFC ${path}: frontmatter number '${mod.metadata.number}' disagrees with filename-derived '${number}'`
    );
  }
  if (mod.metadata.id !== stem) {
    throw new Error(`RFC ${path}: frontmatter id '${mod.metadata.id}' disagrees with filename '${stem}'`);
  }
  const existing = numberToPath.get(number);
  if (existing !== undefined) {
    throw new Error(`Duplicate RFC number ${number}: ${existing} and ${path}`);
  }
  numberToPath.set(number, path);
}

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
