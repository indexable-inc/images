import { describe, expect, test } from 'vitest';
import { rubricTotal, validateRubric, type RubricDimension, type RubricItem } from './rubric';

const dimensions: RubricDimension[] = [
  { key: 'a', label: 'a', meaning: 'first' },
  { key: 'b', label: 'b', meaning: 'second' }
];

function item(a: number, b: number): RubricItem {
  return {
    id: 'x',
    label: 'x',
    detail: 'an item',
    cells: {
      a: { value: a, why: 'because a' },
      b: { value: b, why: 'because b' }
    }
  };
}

describe('rubric', () => {
  test('totals sum the declared dimensions', () => {
    expect(rubricTotal(dimensions, item(2, 1))).toBe(3);
  });

  test('rejects out-of-range and unjustified cells', () => {
    expect(() => {
      validateRubric(dimensions, [item(3, 0)], 2);
    }).toThrow(/0-2/);
    const missingWhy = item(1, 1);
    missingWhy.cells.a = { value: 1, why: '' };
    expect(() => {
      validateRubric(dimensions, [missingWhy], 2);
    }).toThrow(/why/);
    expect(() => {
      validateRubric(dimensions, [item(2, 0)], 2);
    }).not.toThrow();
  });

  test('missing dimension is an error, not a silent zero', () => {
    const partial = item(1, 1);
    delete partial.cells.b;
    expect(() => rubricTotal(dimensions, partial)).toThrow(/missing rubric cell 'b'/);
  });
});
