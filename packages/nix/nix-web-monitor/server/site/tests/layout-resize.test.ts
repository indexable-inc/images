// Splitter-drag clamping in `resizeSizes`, the pure core of
// `DockState.resizeSplit`. The regression of note: with a large hidden
// sibling stored between two visible panes, the minimum-size floor must scale
// by the visible share, or the divider is pinned inside a sliver around the
// panes' stored absolute fractions.

import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { parseLayout, resizeSizes } from '../src/lib/panes/layout.svelte.ts';
import type { DockLayout } from '../src/lib/panes/types.ts';

describe('resizeSizes', () => {
  const cases: {
    name: string;
    sizes: number[];
    left: number;
    right: number;
    delta: number;
    visibleShare: number;
    expected: number[];
  }[] = [
    {
      name: 'moves the boundary between adjacent visible panes',
      sizes: [0.5, 0.5],
      left: 0,
      right: 1,
      delta: 0.2,
      visibleShare: 1,
      expected: [0.7, 0.3]
    },
    {
      name: 'clamps a fully visible pair at MIN_FRACTION',
      sizes: [0.5, 0.5],
      left: 0,
      right: 1,
      delta: 0.6,
      visibleShare: 1,
      expected: [0.92, 0.08]
    },
    {
      // The hidden-sibling regression: sizes [0.1, 0.8, 0.1] with the middle
      // hidden renders the outer panes 50/50. An absolute MIN_FRACTION floor
      // confined each pane to [0.08, 0.12] (40-60% of the rendered extent);
      // scaled by the visible share (0.2) the floor is 0.016, i.e. 8% of what
      // the user sees, and the hidden pane's stored 0.8 stays untouched.
      name: 'resizes across the visible share past a hidden sibling',
      sizes: [0.1, 0.8, 0.1],
      left: 0,
      right: 2,
      delta: 0.2,
      visibleShare: 0.2,
      expected: [0.184, 0.8, 0.016]
    },
    {
      name: 'clamps the same way dragging toward the left pane',
      sizes: [0.1, 0.8, 0.1],
      left: 0,
      right: 2,
      delta: -0.2,
      visibleShare: 0.2,
      expected: [0.016, 0.8, 0.184]
    },
    {
      name: 'applies an in-range hidden-sibling drag exactly',
      sizes: [0.1, 0.8, 0.1],
      left: 0,
      right: 2,
      delta: 0.05,
      visibleShare: 0.2,
      expected: [0.15, 0.8, 0.05]
    },
    {
      // Normalization can leave a pair whose combined share is below two
      // floors (e.g. [0.075, 0.075, 0.85] after clamping); capping the floor
      // at the pair midpoint keeps the clamp range from inverting, so a drag
      // at most equalizes the pair instead of conjuring movement.
      name: 'never inverts when the pair sits below two floors',
      sizes: [0.075, 0.075, 0.85],
      left: 0,
      right: 1,
      delta: 0.5,
      visibleShare: 1,
      expected: [0.075, 0.075, 0.85]
    },
    {
      name: 'ignores out-of-range indices',
      sizes: [0.5, 0.5],
      left: 0,
      right: 5,
      delta: 0.2,
      visibleShare: 1,
      expected: [0.5, 0.5]
    }
  ];

  for (const { name, sizes, left, right, delta, visibleShare, expected } of cases) {
    it(name, () => {
      const mutated = [...sizes];
      resizeSizes(mutated, left, right, delta, visibleShare);
      assert.equal(mutated.length, expected.length);
      for (const [index, value] of expected.entries()) {
        assert.ok(
          Math.abs(mutated[index] - value) < 1e-9,
          `sizes[${String(index)}] = ${String(mutated[index])}, expected ${String(value)}`
        );
      }
      // Redistribution is zero-sum: the split's total never drifts.
      const total = (values: number[]): number => values.reduce((sum, size) => sum + size, 0);
      assert.ok(Math.abs(total(mutated) - total(sizes)) < 1e-9, 'total is preserved');
    });
  }
});

// Duplicate-id hygiene in `parseLayout`, the persisted-blob parser: pane ids
// key the render's `{#each}` blocks and the loaded layout paints once before
// the dock's first `reconcile` pass, so a stale or hand-edited blob repeating
// an id must be deduped synchronously at load (first mention wins).
describe('parseLayout', () => {
  const group = (
    tabs: string[],
    active: string | null = tabs.at(0) ?? null
  ): { kind: 'group'; tabs: string[]; active: string | null; collapsed: boolean } => ({
    kind: 'group',
    tabs,
    active,
    collapsed: false
  });
  const float = (id: string): { id: string; x: number; y: number; width: number; height: number } => ({
    id,
    x: 10,
    y: 20,
    width: 300,
    height: 200
  });

  const cases: { name: string; stored: unknown; expected: DockLayout | null }[] = [
    {
      name: 'passes a well-formed layout through unchanged',
      stored: {
        version: 1,
        root: {
          kind: 'split',
          direction: 'row',
          sizes: [0.5, 0.5],
          children: [group(['logs']), group(['builds'])]
        },
        floating: [float('summary')]
      },
      expected: {
        root: {
          kind: 'split',
          direction: 'row',
          sizes: [0.5, 0.5],
          children: [group(['logs']), group(['builds'])]
        },
        floating: [float('summary')]
      }
    },
    {
      name: 'dedupes a tab repeated within one group',
      stored: { version: 1, root: group(['logs', 'logs']), floating: [] },
      expected: { root: group(['logs']), floating: [] }
    },
    {
      name: 'dedupes a tab repeated across split children, first mention wins',
      stored: {
        version: 1,
        root: {
          kind: 'split',
          direction: 'column',
          sizes: [0.5, 0.5],
          children: [group(['logs']), group(['logs', 'builds'])]
        },
        floating: []
      },
      expected: {
        root: {
          kind: 'split',
          direction: 'column',
          sizes: [0.5, 0.5],
          children: [group(['logs']), group(['builds'])]
        },
        floating: []
      }
    },
    {
      name: 'repoints active when its tab was a dropped duplicate',
      stored: {
        version: 1,
        root: {
          kind: 'split',
          direction: 'row',
          sizes: [0.5, 0.5],
          children: [group(['logs']), group(['logs', 'builds'], 'logs')]
        },
        floating: []
      },
      expected: {
        root: {
          kind: 'split',
          direction: 'row',
          sizes: [0.5, 0.5],
          children: [group(['logs']), group(['builds'], 'builds')]
        },
        floating: []
      }
    },
    {
      name: 'drops a floating window shadowing a docked tab',
      stored: { version: 1, root: group(['logs']), floating: [float('logs'), float('summary')] },
      expected: { root: group(['logs']), floating: [float('summary')] }
    },
    {
      name: 'dedupes floating windows repeating an id',
      stored: { version: 1, root: group(['logs']), floating: [float('summary'), float('summary')] },
      expected: { root: group(['logs']), floating: [float('summary')] }
    },
    {
      name: 'still rejects a version mismatch outright',
      stored: { version: 0, root: group(['logs']), floating: [] },
      expected: null
    }
  ];

  for (const { name, stored, expected } of cases) {
    it(name, () => {
      assert.deepEqual(parseLayout(stored), expected);
    });
  }
});
