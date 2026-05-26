import { describe, expect, test } from 'vitest';
import type { Component } from 'svelte';
import { inlineTitleHtml, plainText, siteUpdates, updateScript } from './updates';

const noopComponent = (() => null) as unknown as Component;

describe('inlineTitleHtml', () => {
  test('wraps backtick spans in <code>', () => {
    expect(inlineTitleHtml('`nix run .#site` previews locally')).toBe(
      '<code>nix run .#site</code> previews locally'
    );
  });

  test('escapes raw HTML before any backtick substitution', () => {
    const out = inlineTitleHtml('<script>alert(1)</script>');
    expect(out).not.toMatch(/<script/i);
    expect(out).toContain('&lt;script&gt;');
  });

  test('handles titles with no backticks', () => {
    expect(inlineTitleHtml('Self-hosted OpenTelemetry stack module')).toBe(
      'Self-hosted OpenTelemetry stack module'
    );
  });
});

describe('plainText', () => {
  test('strips inline code, bold, italic, and link syntax', () => {
    expect(plainText('use `cmd`, **strong**, *em*, and [text](https://x)')).toBe(
      'use cmd, strong, em, and text'
    );
  });

  test('drops fenced code blocks', () => {
    expect(plainText('before\n```\ncode\nblock\n```\nafter')).toBe('before after');
  });
});

describe('updateScript', () => {
  test('joins title and flattened body for RSS descriptions', () => {
    const script = updateScript({
      id: 'demo',
      postedAt: '2026-05-26T01:22:16-07:00',
      title: 'a `cmd` arrived',
      component: noopComponent,
      rawBody: 'It does `things` well.',
      links: [],
      tags: []
    });
    expect(script).toBe('a cmd arrived. It does things well.');
  });
});

describe('siteUpdates', () => {
  test('every entry has the required fields', () => {
    for (const update of siteUpdates) {
      expect(update.id).toMatch(/^[a-z][a-z0-9-]+$/);
      expect(Number.isFinite(Date.parse(update.postedAt))).toBe(true);
      expect(update.title.length).toBeGreaterThan(0);
      expect(typeof update.component).toBe('function');
      expect(Array.isArray(update.links)).toBe(true);
      expect(Array.isArray(update.tags)).toBe(true);
    }
  });

  test('tags are lowercased slugs', () => {
    for (const update of siteUpdates) {
      for (const tag of update.tags) {
        expect(tag).toMatch(/^[a-z][a-z0-9-]*$/);
      }
    }
  });

  test('entries are ordered newest first', () => {
    const times = siteUpdates.map((u) => Date.parse(u.postedAt));
    const sorted = [...times].sort((a, b) => b - a);
    expect(times).toEqual(sorted);
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
