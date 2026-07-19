import type { Component } from 'svelte';
import { validateScores, type Score, type ScoreDimension } from './scores';

// Ordered by weight: from a vibe someone wrote down to a design the whole
// repo depends on, plus the terminal states. The index badge colors ramp
// along this ladder.
export const rfcStatuses = [
  'Sketch',
  'Draft',
  'Input wanted',
  'Last call',
  'Accepted',
  'Load-bearing',
  'Rejected',
  'Withdrawn',
  'Superseded'
] as const;
export type RfcStatus = (typeof rfcStatuses)[number];

// Score axes every RFC carries in frontmatter under `scores:`, each with a
// 1-10 value and a why. The chart on /rfcs plots any two against each other.
export const rfcDimensions: readonly ScoreDimension[] = [
  {
    key: 'ambition',
    label: 'Ambition',
    low: 'Incremental',
    high: 'Moonshot',
    meaning:
      'How far past current practice the proposal reaches: a tweak to something that exists, or a bet that changes what the repo is.'
  },
  {
    key: 'impact',
    label: 'Impact',
    low: 'Marginal',
    high: 'Transformative',
    meaning: 'How much better the repo gets if this lands, summed across everyone who works in it.'
  },
  {
    key: 'effort',
    label: 'Effort',
    low: 'Weekend',
    high: 'Quarter',
    meaning: 'Wall-clock and complexity to implement fully, not just to prototype.'
  },
  {
    key: 'risk',
    label: 'Risk',
    low: 'Safe',
    high: 'Hairy',
    meaning:
      'What breaks, or what the repo is stuck carrying, if the bet turns out wrong: blast radius plus reversibility.'
  },
  {
    key: 'maturity',
    label: 'Maturity',
    low: 'Rough',
    high: 'Battle-tested',
    meaning:
      'How hardened the implementation is today: unbuilt or rough sketch code at the low end, a daily-driven battle-tested system at the high end. Orthogonal to API stability.'
  },
  {
    key: 'leverage',
    label: 'Leverage',
    low: 'One-off',
    high: 'Flywheel',
    meaning:
      'Whether it compounds: a one-off improvement, a multiplier that helps everything else, or a flywheel where each use makes the next cheaper.'
  },
  {
    key: 'taste',
    label: 'Taste',
    low: 'Mechanical',
    high: 'Judgment',
    meaning:
      'How much tasteful human input the work needs. Low means the path is straightforward and an advanced model can execute it; high means the hard part is knowing what good looks like, and human judgment moves the outcome.'
  }
];

export type RfcMeta = {
  id: string;
  number: string;
  // Markdown source: backticks for inline code.
  title: string;
  status: RfcStatus;
  authors: string;
  // YYYY-MM-DD.
  created: string;
  updated: string;
  // 1-10 scores with rationale for the /rfcs chart. Seed estimates; tune via PR.
  scores: Record<string, Score>;
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
  validateScores(`RFC ${path}`, rfcDimensions, mod.metadata.scores);
  if (!(rfcStatuses as readonly string[]).includes(mod.metadata.status)) {
    throw new Error(
      `RFC ${path}: unknown status '${mod.metadata.status}'; expected one of ${rfcStatuses.join(', ')}`
    );
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
