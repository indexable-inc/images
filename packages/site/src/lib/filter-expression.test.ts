import { describe, expect, test } from 'vitest';
import { parseFilter } from './filter-expression';

function matches(expr: string, tags: string[]): boolean {
  const result = parseFilter(expr);
  if (!result.ok) throw new Error(`parse failed: ${result.error}`);
  return result.matches(tags);
}

describe('parseFilter', () => {
  test('empty input matches everything', () => {
    expect(matches('', [])).toBe(true);
    expect(matches('   ', ['nix'])).toBe(true);
  });

  test('single tag', () => {
    expect(matches('nix', ['nix', 'cli'])).toBe(true);
    expect(matches('nix', ['rust'])).toBe(false);
  });

  test('explicit AND', () => {
    expect(matches('nix & cli', ['nix', 'cli'])).toBe(true);
    expect(matches('nix & cli', ['nix'])).toBe(false);
  });

  test('implicit AND', () => {
    expect(matches('nix cli', ['nix', 'cli'])).toBe(true);
    expect(matches('nix cli', ['nix'])).toBe(false);
  });

  test('OR', () => {
    expect(matches('rust | zig', ['zig'])).toBe(true);
    expect(matches('rust | zig', ['nix'])).toBe(false);
  });

  test('NOT', () => {
    expect(matches('!testing', ['nix'])).toBe(true);
    expect(matches('!testing', ['nix', 'testing'])).toBe(false);
  });

  test('precedence: AND binds tighter than OR', () => {
    // nix & rust | zig === (nix & rust) | zig
    expect(matches('nix & rust | zig', ['zig'])).toBe(true);
    expect(matches('nix & rust | zig', ['nix', 'rust'])).toBe(true);
    expect(matches('nix & rust | zig', ['nix'])).toBe(false);
  });

  test('parentheses override precedence', () => {
    expect(matches('nix & (rust | zig)', ['nix', 'zig'])).toBe(true);
    expect(matches('nix & (rust | zig)', ['zig'])).toBe(false);
  });

  test('NOT binds tighter than AND', () => {
    // !a & b means (!a) & b
    expect(matches('!a & b', ['b'])).toBe(true);
    expect(matches('!a & b', ['a', 'b'])).toBe(false);
  });

  test('the example from the user message', () => {
    // abc & xyz (asdsad | asdasd)
    const tags = ['abc', 'xyz', 'asdsad'];
    expect(matches('abc & xyz (asdsad | asdasd)', tags)).toBe(true);
    expect(matches('abc & xyz (asdsad | asdasd)', ['abc', 'xyz'])).toBe(false);
  });

  test('tag names are lowercased', () => {
    expect(matches('Nix', ['nix'])).toBe(true);
  });

  test('reports an error on bad syntax', () => {
    const result = parseFilter('nix &');
    expect(result.ok).toBe(false);
  });

  test('reports an error on unmatched parens', () => {
    const result = parseFilter('(nix');
    expect(result.ok).toBe(false);
  });

  test('reports an error on unexpected character', () => {
    const result = parseFilter('nix @ rust');
    expect(result.ok).toBe(false);
  });
});
