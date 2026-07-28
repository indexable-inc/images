import { describe, expect, test } from 'vitest';
import {
  compileSearch,
  searchTokens,
  tagOptions,
  wordAt
} from './filter-expression';

const TAGS = ['nix', 'rust', 'site', 'testing', 'interesting', 'cli', 'zig'];

function matches(
  input: string,
  candidate: { tags?: string[]; text?: string }
): boolean {
  return compileSearch(input, TAGS)({
    tags: candidate.tags ?? [],
    text: candidate.text ?? ''
  });
}

describe('compileSearch', () => {
  test('empty input matches everything', () => {
    expect(matches('', {})).toBe(true);
    expect(matches('   ', { tags: ['nix'] })).toBe(true);
  });

  test('a known tag filters by tag', () => {
    expect(matches('nix', { tags: ['nix', 'cli'] })).toBe(true);
    expect(matches('nix', { tags: ['rust'] })).toBe(false);
  });

  test('a prefix of a known tag filters like the tag', () => {
    expect(matches('rus', { tags: ['rust'] })).toBe(true);
    expect(matches('rus', { tags: ['nix'] })).toBe(false);
  });

  test('tag words are case-insensitive', () => {
    expect(matches('Nix', { tags: ['nix'] })).toBe(true);
  });

  test('adjacent tags AND together', () => {
    expect(matches('nix cli', { tags: ['nix', 'cli'] })).toBe(true);
    expect(matches('nix cli', { tags: ['nix'] })).toBe(false);
    expect(matches('nix & cli', { tags: ['nix', 'cli'] })).toBe(true);
  });

  test('unknown words full-text match, case-insensitively', () => {
    expect(matches('golden', { text: 'The Golden snapshot pipeline' })).toBe(true);
    expect(matches('golden', { text: 'nothing here' })).toBe(false);
  });

  test('tags and free text combine with AND', () => {
    const entry = { tags: ['rust'], text: 'golden snapshot restore' };
    expect(matches('rust golden', entry)).toBe(true);
    expect(matches('rust missing', entry)).toBe(false);
    expect(matches('zig golden', entry)).toBe(false);
  });

  test('OR', () => {
    expect(matches('rust | zig', { tags: ['zig'] })).toBe(true);
    expect(matches('rust | zig', { tags: ['nix'] })).toBe(false);
  });

  test('NOT', () => {
    expect(matches('!testing', { tags: ['nix'] })).toBe(true);
    expect(matches('!testing', { tags: ['nix', 'testing'] })).toBe(false);
  });

  test('precedence: AND binds tighter than OR', () => {
    // nix & rust | zig === (nix & rust) | zig
    expect(matches('nix & rust | zig', { tags: ['zig'] })).toBe(true);
    expect(matches('nix & rust | zig', { tags: ['nix', 'rust'] })).toBe(true);
    expect(matches('nix & rust | zig', { tags: ['nix'] })).toBe(false);
  });

  test('parentheses override precedence', () => {
    expect(matches('nix & (rust | zig)', { tags: ['nix', 'zig'] })).toBe(true);
    expect(matches('nix & (rust | zig)', { tags: ['zig'] })).toBe(false);
  });

  test('NOT applies to free text too', () => {
    expect(matches('!golden', { text: 'a golden snapshot' })).toBe(false);
    expect(matches('!golden', { text: 'plain' })).toBe(true);
  });

  test('incomplete syntax never errors: trailing operator is dropped', () => {
    expect(matches('nix &', { tags: ['nix'] })).toBe(true);
    expect(matches('nix |', { tags: ['nix'] })).toBe(true);
    expect(matches('nix !', { tags: ['nix'] })).toBe(true);
    expect(matches('nix &', { tags: ['rust'] })).toBe(false);
  });

  test('unmatched parens never error', () => {
    expect(matches('(nix', { tags: ['nix'] })).toBe(true);
    expect(matches('nix)', { tags: ['nix'] })).toBe(true);
    expect(matches('(nix', { tags: ['rust'] })).toBe(false);
  });

  test('arbitrary characters are free text, not errors', () => {
    expect(matches('v2.3', { text: 'ships v2.3 today' })).toBe(true);
    expect(matches('foo@bar', { text: 'mail foo@bar now' })).toBe(true);
    expect(matches('foo@bar', { text: 'foo bar' })).toBe(false);
  });
});

describe('searchTokens', () => {
  test('spans concatenate back to the input exactly', () => {
    const input = '  nix & (rus | zig)  golden!';
    const joined = searchTokens(input, TAGS)
      .map((t) => t.text)
      .join('');
    expect(joined).toBe(input);
  });

  test('classifies tag prefixes as tags and unknown words as text', () => {
    const kinds = searchTokens('rus golden', TAGS).map((t) => t.kind);
    expect(kinds).toEqual(['tag', 'space', 'text']);
  });

  test('classifies operators and parens', () => {
    const kinds = searchTokens('!(a|b)&c', TAGS).map((t) => t.kind);
    expect(kinds).toEqual([
      'op-not',
      'paren',
      'text',
      'op-or',
      'text',
      'paren',
      'op-and',
      'tag' // `c` prefixes `cli`
    ]);
  });
});

describe('wordAt', () => {
  test('finds the word around the caret', () => {
    expect(wordAt('nix rust', 5)).toEqual({ start: 4, end: 8, word: 'rust' });
    expect(wordAt('nix rust', 8)).toEqual({ start: 4, end: 8, word: 'rust' });
  });

  test('operators bound words', () => {
    expect(wordAt('nix&rust', 6)).toEqual({ start: 4, end: 8, word: 'rust' });
  });

  test('returns an empty word between separators', () => {
    expect(wordAt('nix ', 4)).toEqual({ start: 4, end: 4, word: '' });
  });
});

describe('tagOptions', () => {
  test('counts entries per tag, most-used first, ties by name', () => {
    const options = tagOptions([
      { tags: ['nix', 'site'] },
      { tags: ['nix'] },
      { tags: ['cli'] }
    ]);
    expect(options).toEqual([
      { name: 'nix', count: 2 },
      { name: 'cli', count: 1 },
      { name: 'site', count: 1 }
    ]);
  });
});
