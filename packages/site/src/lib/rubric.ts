// Generic small-integer rubric vocabulary. A catalog declares dimensions,
// scores each item 0..max per dimension with a value AND a why so the matrix
// can be critiqued in review, and feeds RubricGrid.svelte. This complements
// scores.ts: that file is a 1-10 two-axis vocabulary for the ScoreChart
// scatter; a rubric is a summed checklist rendered as a matrix.

export type RubricCell = {
  value: number;
  why: string;
};

export type RubricDimension = {
  key: string;
  label: string;
  // What earns points on this dimension; rendered in the hover status line.
  meaning: string;
};

export type RubricItem = {
  id: string;
  label: string;
  detail: string;
  href?: string;
  cells: Record<string, RubricCell>;
};

// `cells` is Partial so the lookup admits undefined under any tsconfig;
// consumers compile with noUncheckedIndexedAccess, this app without.
export function rubricCellOf(
  owner: string,
  cells: Partial<Record<string, RubricCell>>,
  key: string
): RubricCell {
  const cell = cells[key];
  if (cell === undefined) {
    throw new Error(`${owner}: missing rubric cell '${key}'`);
  }
  return cell;
}

export function rubricTotal(dimensions: readonly RubricDimension[], item: RubricItem): number {
  return dimensions.reduce((sum, dim) => sum + rubricCellOf(item.id, item.cells, dim.key).value, 0);
}

export function validateRubric(
  dimensions: readonly RubricDimension[],
  items: readonly RubricItem[],
  maxCell: number
): void {
  for (const item of items) {
    for (const { key } of dimensions) {
      const cell = rubricCellOf(item.id, item.cells, key);
      if (!Number.isInteger(cell.value) || cell.value < 0 || cell.value > maxCell) {
        throw new Error(
          `${item.id}: rubric cell '${key}' must be an integer 0-${String(maxCell)}, got ${String(cell.value)}`
        );
      }
      if (typeof cell.why !== 'string' || cell.why.length === 0) {
        throw new Error(`${item.id}: rubric cell '${key}' needs a why so the number can be critiqued`);
      }
    }
  }
}
