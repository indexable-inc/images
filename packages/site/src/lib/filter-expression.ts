// Todoist-style search over the updates feed.
//
//   rust golden          // tag `rust` AND free text "golden"
//   rus                  // a prefix of a known tag filters like the tag
//   nix & (rust | zig)   // boolean power syntax still available
//
// Words that are prefixes of known tags filter by tag; every other word is
// free text, substring-matched against an entry's title + raw body. Adjacent
// terms AND together. `&`, `|`, `!`, and parentheses survive as power syntax
// with precedence NOT > AND > OR, but nothing the user types is ever an
// error: incomplete operators and stray parens are dropped and the query is
// whatever did parse. Empty input matches everything.

export type TagOption = { name: string; count: number };

export type SearchCandidate = {
  // Lowercased tag slugs, as loaded by `updates.ts`.
  tags: readonly string[];
  // Searchable text (title + raw body). Matched case-insensitively.
  text: string;
};

// Spans for inline highlighting. They concatenate back to the input exactly,
// so an overlay painted from them stays aligned with the real <input> text.
export type SearchToken = {
  text: string;
  kind: 'tag' | 'text' | 'op-and' | 'op-or' | 'op-not' | 'paren' | 'space';
};

type RawToken =
  | { kind: 'word'; text: string }
  | { kind: 'op-and' | 'op-or' | 'op-not' | 'lparen' | 'rparen' | 'space'; text: string };

// Word characters are anything that is not whitespace or operator syntax, so
// arbitrary free text (`foo@bar`, `v2.3`) stays one searchable word.
const BOUNDARY = /[\s&|!()]/;

function tokenize(input: string): RawToken[] {
  const out: RawToken[] = [];
  let i = 0;
  while (i < input.length) {
    const c = input.charAt(i);
    if (/\s/.test(c)) {
      let j = i;
      while (j < input.length && /\s/.test(input.charAt(j))) j++;
      out.push({ kind: 'space', text: input.slice(i, j) });
      i = j;
    } else if (c === '&') {
      out.push({ kind: 'op-and', text: c });
      i++;
    } else if (c === '|') {
      out.push({ kind: 'op-or', text: c });
      i++;
    } else if (c === '!') {
      out.push({ kind: 'op-not', text: c });
      i++;
    } else if (c === '(') {
      out.push({ kind: 'lparen', text: c });
      i++;
    } else if (c === ')') {
      out.push({ kind: 'rparen', text: c });
      i++;
    } else {
      let j = i;
      while (j < input.length && !BOUNDARY.test(input.charAt(j))) j++;
      out.push({ kind: 'word', text: input.slice(i, j) });
      i = j;
    }
  }
  return out;
}

// A word acts as a tag filter when it is a prefix of at least one known tag
// (a full tag name is its own prefix). This is also exactly what the inline
// pill highlight shows, so what lights up is what filters.
export function isTagWord(word: string, knownTags: readonly string[]): boolean {
  if (word.length === 0) return false;
  const lower = word.toLowerCase();
  return knownTags.some((tag) => tag.startsWith(lower));
}

export function searchTokens(input: string, knownTags: readonly string[]): SearchToken[] {
  return tokenize(input).map((token): SearchToken => {
    if (token.kind === 'word') {
      return { text: token.text, kind: isTagWord(token.text, knownTags) ? 'tag' : 'text' };
    }
    if (token.kind === 'lparen' || token.kind === 'rparen') {
      return { text: token.text, kind: 'paren' };
    }
    return { text: token.text, kind: token.kind };
  });
}

// The word under the caret, for autocomplete. Boundaries are whitespace and
// operator characters, matching `tokenize`.
export function wordAt(
  input: string,
  caret: number
): { start: number; end: number; word: string } {
  let start = caret;
  while (start > 0 && !BOUNDARY.test(input.charAt(start - 1))) start--;
  let end = caret;
  while (end < input.length && !BOUNDARY.test(input.charAt(end))) end++;
  return { start, end, word: input.slice(start, end) };
}

// Tags with entry counts for the autocomplete dropdown, most-used first.
export function tagOptions(entries: readonly { tags: readonly string[] }[]): TagOption[] {
  const counts = new Map<string, number>();
  for (const entry of entries) {
    for (const tag of entry.tags) counts.set(tag, (counts.get(tag) ?? 0) + 1);
  }
  return [...counts]
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
}

type Node =
  | { kind: 'tag'; word: string }
  | { kind: 'text'; word: string }
  | { kind: 'not'; expr: Node }
  | { kind: 'and'; left: Node; right: Node }
  | { kind: 'or'; left: Node; right: Node };

// Tolerant recursive descent: every rule returns null instead of throwing
// when its operand is missing, and callers drop the incomplete operator.
class Parser {
  private pos = 0;
  private depth = 0;
  private readonly toks: RawToken[];

  constructor(
    toks: RawToken[],
    private readonly knownTags: readonly string[]
  ) {
    this.toks = toks.filter((t) => t.kind !== 'space');
  }

  private peek(): RawToken | undefined {
    return this.toks[this.pos];
  }

  private next(): RawToken | undefined {
    return this.toks[this.pos++];
  }

  parseOr(): Node | null {
    let left = this.parseAnd();
    while (this.peek()?.kind === 'op-or') {
      this.next();
      const right = this.parseAnd();
      left = left && right ? { kind: 'or', left, right } : (left ?? right);
    }
    return left;
  }

  private parseAnd(): Node | null {
    let left: Node | null = null;
    for (;;) {
      const t = this.peek();
      if (!t || t.kind === 'op-or') break;
      if (t.kind === 'rparen') {
        if (this.depth > 0) break;
        this.next(); // stray ')' with no open group: noise
        continue;
      }
      if (t.kind === 'op-and') {
        this.next(); // explicit AND; adjacency already means AND
        continue;
      }
      const term = this.parseNot();
      if (term) left = left ? { kind: 'and', left, right: term } : term;
    }
    return left;
  }

  private parseNot(): Node | null {
    if (this.peek()?.kind === 'op-not') {
      this.next();
      const inner = this.parseNot();
      return inner ? { kind: 'not', expr: inner } : null;
    }
    return this.parseAtom();
  }

  private parseAtom(): Node | null {
    const t = this.next();
    if (!t) return null;
    if (t.kind === 'word') {
      const word = t.text.toLowerCase();
      return isTagWord(word, this.knownTags)
        ? { kind: 'tag', word }
        : { kind: 'text', word };
    }
    if (t.kind === 'lparen') {
      this.depth++;
      const inner = this.parseOr();
      this.depth--;
      if (this.peek()?.kind === 'rparen') this.next();
      return inner;
    }
    return null; // unreachable: parseAnd filters every other kind
  }
}

function evaluate(node: Node, tags: readonly string[], text: string): boolean {
  switch (node.kind) {
    case 'tag':
      return tags.some((tag) => tag.startsWith(node.word));
    case 'text':
      return text.includes(node.word);
    case 'not':
      return !evaluate(node.expr, tags, text);
    case 'and':
      return evaluate(node.left, tags, text) && evaluate(node.right, tags, text);
    case 'or':
      return evaluate(node.left, tags, text) || evaluate(node.right, tags, text);
  }
}

export function compileSearch(
  input: string,
  knownTags: readonly string[]
): (candidate: SearchCandidate) => boolean {
  const node = new Parser(tokenize(input), knownTags).parseOr();
  if (!node) return () => true;
  return (candidate) => evaluate(node, candidate.tags, candidate.text.toLowerCase());
}
