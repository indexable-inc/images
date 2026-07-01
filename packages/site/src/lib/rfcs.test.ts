import { describe, expect, test } from 'vitest';
import { findRfc, rfcEntries, rfcs } from './rfcs';

describe('rfcs', () => {
  test('every entry has the required fields', () => {
    for (const rfc of rfcs) {
      expect(rfc.id).toMatch(/^\d{4}-[a-z0-9-]+$/);
      expect(rfc.number).toMatch(/^\d{4}$/);
      expect(rfc.id.startsWith(rfc.number)).toBe(true);
      expect(rfc.title.length).toBeGreaterThan(0);
      expect(rfc.status.length).toBeGreaterThan(0);
      expect(typeof rfc.component).toBe('function');
    }
  });

  test('nullable frontmatter fields are string or null, never undefined', () => {
    for (const rfc of rfcs) {
      for (const field of [rfc.trackingIssue, rfc.supersedes, rfc.supersededBy] as const) {
        expect(field === null || typeof field === 'string').toBe(true);
      }
    }
  });

  test('entries are ordered ascending by RFC number', () => {
    const numbers = rfcs.map((r) => r.number);
    const sorted = [...numbers].sort((a, b) => a.localeCompare(b));
    expect(numbers).toEqual(sorted);
  });

  test('ids are unique', () => {
    const ids = rfcs.map((r) => r.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  test('the template is excluded from the listing but still resolvable by id', () => {
    expect(rfcs.some((r) => r.template)).toBe(false);
    const template = findRfc('0000-template');
    expect(template?.template).toBe(true);
  });

  test('rfcEntries covers every id findRfc can resolve, including the template', () => {
    const entryIds = rfcEntries().map((e) => e.id);
    expect(new Set(entryIds).size).toBe(entryIds.length);
    for (const id of entryIds) {
      expect(findRfc(id)).toBeDefined();
    }
    expect(entryIds).toContain('0000-template');
  });

  test('findRfc returns undefined for an unknown id', () => {
    expect(findRfc('9999-does-not-exist')).toBeUndefined();
  });
});
