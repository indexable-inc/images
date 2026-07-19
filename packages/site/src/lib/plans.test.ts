import { describe, expect, test } from 'vitest';
import { findPlan, planEntries, plans } from './plans';

describe('plans', () => {
  test('every entry has the required fields', () => {
    for (const plan of plans) {
      expect(plan.id).toMatch(/^\d{4}-[a-z0-9-]+$/);
      expect(plan.number).toMatch(/^\d{4}$/);
      expect(plan.id.startsWith(plan.number)).toBe(true);
      expect(plan.title.length).toBeGreaterThan(0);
      expect(plan.status.length).toBeGreaterThan(0);
      expect(typeof plan.component).toBe('function');
    }
  });

  test('nullable frontmatter fields are string or null, never undefined', () => {
    for (const plan of plans) {
      for (const field of [plan.trackingIssue, plan.supersedes, plan.supersededBy] as const) {
        expect(field === null || typeof field === 'string').toBe(true);
      }
    }
  });

  test('entries are ordered ascending by Plan number', () => {
    const numbers = plans.map((r) => r.number);
    const sorted = [...numbers].sort((a, b) => a.localeCompare(b));
    expect(numbers).toEqual(sorted);
  });

  test('ids are unique', () => {
    const ids = plans.map((r) => r.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  test('the template is excluded from the listing but still resolvable by id', () => {
    expect(plans.some((r) => r.template)).toBe(false);
    const template = findPlan('0000-template');
    expect(template?.template).toBe(true);
  });

  test('planEntries covers every id findPlan can resolve, including the template', () => {
    const entryIds = planEntries().map((e) => e.id);
    expect(new Set(entryIds).size).toBe(entryIds.length);
    for (const id of entryIds) {
      expect(findPlan(id)).toBeDefined();
    }
    expect(entryIds).toContain('0000-template');
  });

  test('findPlan returns undefined for an unknown id', () => {
    expect(findPlan('9999-does-not-exist')).toBeUndefined();
  });

  test('plan numbers are unique across every entry, including the template', () => {
    const numbers = planEntries().map((e) => findPlan(e.id)?.number);
    expect(new Set(numbers).size).toBe(numbers.length);
  });
});
