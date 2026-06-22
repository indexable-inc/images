// A tiny flecs-style boolean tag filter.
//
//   nix                  // single tag
//   nix & cli            // both
//   nix cli              // implicit AND between adjacent terms
//   nix | rust           // either
//   !testing             // absent
//   nix & (rust | zig)   // grouped
//
// Precedence: NOT > AND > OR. Empty input matches everything.

type Token =
  | { kind: 'tag'; name: string }
  | { kind: 'and' }
  | { kind: 'or' }
  | { kind: 'not' }
  | { kind: 'lparen' }
  | { kind: 'rparen' };

type Node =
  | { kind: 'tag'; name: string }
  | { kind: 'not'; expr: Node }
  | { kind: 'and'; left: Node; right: Node }
  | { kind: 'or'; left: Node; right: Node };

export type FilterExpression =
  | { ok: true; matches: (tags: readonly string[]) => boolean }
  | { ok: false; error: string };

const ALWAYS: FilterExpression = { ok: true, matches: () => true };

export function parseFilter(input: string): FilterExpression {
  const trimmed = input.trim();
  if (trimmed.length === 0) return ALWAYS;

  const tokens = tokenize(trimmed);
  if ('error' in tokens) return { ok: false, error: tokens.error };

  const parser = new Parser(tokens);
  let node: Node;
  try {
    node = parser.parseOr();
  } catch (err) {
    return { ok: false, error: (err as Error).message };
  }
  if (!parser.done()) {
    return { ok: false, error: `unexpected token at position ${String(parser.cursor())}` };
  }

  return {
    ok: true,
    matches: (tags) => evaluate(node, new Set(tags))
  };
}

function tokenize(input: string): Token[] | { error: string } {
  const tokens: Token[] = [];
  let i = 0;
  while (i < input.length) {
    const c = input[i];
    if (/\s/.test(c)) {
      i++;
      continue;
    }
    if (c === '&') {
      tokens.push({ kind: 'and' });
      i++;
      continue;
    }
    if (c === '|') {
      tokens.push({ kind: 'or' });
      i++;
      continue;
    }
    if (c === '!') {
      tokens.push({ kind: 'not' });
      i++;
      continue;
    }
    if (c === '(') {
      tokens.push({ kind: 'lparen' });
      i++;
      continue;
    }
    if (c === ')') {
      tokens.push({ kind: 'rparen' });
      i++;
      continue;
    }
    if (/[a-zA-Z0-9_-]/.test(c)) {
      let j = i;
      while (j < input.length && /[a-zA-Z0-9_-]/.test(input[j])) j++;
      tokens.push({ kind: 'tag', name: input.slice(i, j).toLowerCase() });
      i = j;
      continue;
    }
    return { error: `unexpected character '${c}' at position ${String(i)}` };
  }
  return tokens;
}

class Parser {
  pos = 0;
  constructor(private readonly toks: Token[]) {}

  done(): boolean {
    return this.pos >= this.toks.length;
  }

  cursor(): number {
    return this.pos;
  }

  peek(): Token | undefined {
    return this.toks[this.pos];
  }

  next(): Token | undefined {
    return this.toks[this.pos++];
  }

  parseOr(): Node {
    let left = this.parseAnd();
    while (this.peek()?.kind === 'or') {
      this.next();
      const right = this.parseAnd();
      left = { kind: 'or', left, right };
    }
    return left;
  }

  parseAnd(): Node {
    let left = this.parseNot();
    for (;;) {
      const t = this.peek();
      if (!t) break;
      if (t.kind === 'and') {
        this.next();
      } else if (t.kind === 'tag' || t.kind === 'lparen' || t.kind === 'not') {
        // implicit AND between adjacent terms
      } else {
        break;
      }
      const right = this.parseNot();
      left = { kind: 'and', left, right };
    }
    return left;
  }

  parseNot(): Node {
    if (this.peek()?.kind === 'not') {
      this.next();
      return { kind: 'not', expr: this.parseNot() };
    }
    return this.parseAtom();
  }

  parseAtom(): Node {
    const t = this.next();
    if (!t) throw new Error('unexpected end of expression');
    if (t.kind === 'tag') return { kind: 'tag', name: t.name };
    if (t.kind === 'lparen') {
      const inner = this.parseOr();
      const close = this.next();
      if (close?.kind !== 'rparen') throw new Error("expected ')'");
      return inner;
    }
    throw new Error(`unexpected '${t.kind}' token`);
  }
}

// Tolerant tokenizer for syntax highlighting. Unlike `tokenize`, this never
// fails: any character lands in some span so the rendered overlay always
// matches the input character-for-character. Whitespace is preserved as its
// own span so the spans concatenate back to the original string exactly.
export type HighlightToken = {
  text: string;
  kind: 'tag' | 'op-and' | 'op-or' | 'op-not' | 'paren' | 'space' | 'error';
};

export function highlightExpression(input: string): HighlightToken[] {
  const out: HighlightToken[] = [];
  let i = 0;
  while (i < input.length) {
    const c = input[i];
    if (/\s/.test(c)) {
      let j = i;
      while (j < input.length && /\s/.test(input[j])) j++;
      out.push({ text: input.slice(i, j), kind: 'space' });
      i = j;
    } else if (c === '&') {
      out.push({ text: c, kind: 'op-and' });
      i++;
    } else if (c === '|') {
      out.push({ text: c, kind: 'op-or' });
      i++;
    } else if (c === '!') {
      out.push({ text: c, kind: 'op-not' });
      i++;
    } else if (c === '(' || c === ')') {
      out.push({ text: c, kind: 'paren' });
      i++;
    } else if (/[a-zA-Z0-9_-]/.test(c)) {
      let j = i;
      while (j < input.length && /[a-zA-Z0-9_-]/.test(input[j])) j++;
      out.push({ text: input.slice(i, j), kind: 'tag' });
      i = j;
    } else {
      out.push({ text: c, kind: 'error' });
      i++;
    }
  }
  return out;
}

function evaluate(node: Node, tags: Set<string>): boolean {
  switch (node.kind) {
    case 'tag':
      return tags.has(node.name);
    case 'not':
      return !evaluate(node.expr, tags);
    case 'and':
      return evaluate(node.left, tags) && evaluate(node.right, tags);
    case 'or':
      return evaluate(node.left, tags) || evaluate(node.right, tags);
  }
}
