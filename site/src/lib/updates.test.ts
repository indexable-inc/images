import { describe, expect, test } from 'vitest';
import { plainText, siteUpdates, updateScript } from './updates';

describe('plainText', () => {
  test('strips inline code, bold, italic, and link syntax', () => {
    const out = plainText('use `cmd`, **strong**, *em*, and [text](https://x)');
    expect(out).toBe('use cmd, strong, em, and text');
  });
});

describe('updateScript', () => {
  test('joins title and flattened body for RSS descriptions', () => {
    const script = updateScript({
      id: 'demo',
      date: '2026-05-26',
      title: 'a `cmd` arrived',
      body: 'It does `things` well.',
      links: []
    });
    expect(script).toBe('a `cmd` arrived. It does things well.');
  });
});

describe('siteUpdates', () => {
  test('every entry has the required fields', () => {
    for (const update of siteUpdates) {
      expect(update.id).toMatch(/^[a-z][a-z0-9-]+$/);
      expect(update.date).toMatch(/^\d{4}-\d{2}-\d{2}$/);
      expect(update.title.length).toBeGreaterThan(0);
      expect(update.body.length).toBeGreaterThan(0);
      expect(Array.isArray(update.links)).toBe(true);
    }
  });

  test('entries are ordered newest first', () => {
    const dates = siteUpdates.map((u) => u.date);
    const sorted = [...dates].sort().reverse();
    expect(dates).toEqual(sorted);
  });

  test('link hrefs are absolute https URLs', () => {
    for (const update of siteUpdates) {
      for (const link of update.links) {
        expect(link.href).toMatch(/^https:\/\//);
        expect(link.label.length).toBeGreaterThan(0);
      }
    }
  });

  test('ids are unique', () => {
    const ids = siteUpdates.map((u) => u.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});
