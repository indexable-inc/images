import type { Component } from 'svelte';

export interface Version {
  /** Ordinal parsed from the filename: v0.svx -> 0. */
  ordinal: number;
  /** Short id shown in the picker: "v0". */
  id: string;
  /** Frontmatter title, falling back to the id. */
  title: string;
  /** Optional frontmatter note describing what changed in this version. */
  note?: string;
  /** The compiled svx page. */
  component: Component;
  /** Raw file source; the diff view compares these. */
  source: string;
}

interface VersionModule {
  default: Component;
  metadata?: { title?: string; note?: string };
}

// One file per version. Identity is the filename ordinal: versions are
// immutable once superseded (add v3, never rewrite v1), so the ordinal is a
// stable id and the raw source is the version's full content.
const modules = import.meta.glob<VersionModule>('../versions/v*.svx', {
  eager: true
});
const sources = import.meta.glob<string>('../versions/v*.svx', {
  eager: true,
  query: '?raw',
  import: 'default'
});

export const versions: Version[] = Object.entries(modules)
  .map(([path, mod]) => {
    const match = path.match(/v(\d+)\.svx$/);
    if (!match) throw new Error(`version file must be v<n>.svx: ${path}`);
    const ordinal = Number(match[1]);
    const id = `v${ordinal}`;
    return {
      ordinal,
      id,
      title: mod.metadata?.title ?? id,
      note: mod.metadata?.note,
      component: mod.default,
      source: sources[path]
    };
  })
  .sort((a, b) => a.ordinal - b.ordinal);

if (versions.length === 0) {
  throw new Error('no versions found in src/versions/ (expected v0.svx, ...)');
}
