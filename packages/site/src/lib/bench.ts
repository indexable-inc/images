// Vocabulary for BenchCompareTable.svelte: a cross-model benchmark table
// where provenance is first-class. Every cell carries who produced the
// number (vendor / third party / local system card) and an optional harness
// note; an absent cell renders blank rather than borrowing a number
// measured under a different configuration.

export type BenchSource = 'vendor' | 'third' | 'card';

export type BenchCell = {
  value: string;
  source: BenchSource;
  note?: string;
};

export type BenchColumn = {
  key: string;
  label: string;
};

export type BenchRow = {
  id: string;
  label: string;
  href?: string;
  // Gaming-resistance rubric total for this benchmark, so readers can
  // weight rows; omit for rows the rubric does not cover.
  resistance?: number;
  note?: string;
  cells: Partial<Record<string, BenchCell>>;
};
