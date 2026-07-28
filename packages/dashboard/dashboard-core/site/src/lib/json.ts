// Render a JSON string into a compact, readable DOM tree. Framework-agnostic so
// DataBody.svelte can call it from an effect, the same shape as ansi.ts.
//
// This is the fallback (and `kv`) renderer for data panes: objects become
// dimmed `key value` rows, arrays become indexed rows, primitives render inline
// with a per-type color. A pane whose `body` is not valid JSON shows the raw
// text rather than vanishing.

function leaf(value: string | number | boolean | null): HTMLElement {
  const span = document.createElement('span');
  if (value === null) {
    span.className = 'json-null';
    span.textContent = 'null';
  } else if (typeof value === 'string') {
    span.className = 'json-str';
    span.textContent = value;
  } else if (typeof value === 'boolean') {
    span.className = 'json-bool';
    span.textContent = String(value);
  } else {
    span.className = 'json-num';
    span.textContent = String(value);
  }
  return span;
}

// One `label: value` row. A nested object or array drops onto its own indented
// block under the label; a primitive sits inline beside it.
function row(label: string, value: unknown): HTMLElement {
  const el = document.createElement('div');
  el.className = 'json-row';
  const key = document.createElement('span');
  key.className = 'json-key';
  key.textContent = label;
  el.append(key);
  if (value !== null && typeof value === 'object') {
    el.append(buildNode(value));
  } else {
    el.append(leaf(value as string | number | boolean | null));
  }
  return el;
}

function buildNode(value: unknown): HTMLElement {
  if (Array.isArray(value)) {
    const box = document.createElement('div');
    box.className = 'json-children';
    if (value.length === 0) {
      const empty = document.createElement('span');
      empty.className = 'json-empty';
      empty.textContent = '[]';
      box.append(empty);
      return box;
    }
    value.forEach((item, i) => box.append(row(String(i), item)));
    return box;
  }
  if (value !== null && typeof value === 'object') {
    const box = document.createElement('div');
    box.className = 'json-children';
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 0) {
      const empty = document.createElement('span');
      empty.className = 'json-empty';
      empty.textContent = '{}';
      box.append(empty);
      return box;
    }
    for (const [k, v] of entries) box.append(row(k, v));
    return box;
  }
  // A top-level primitive (rare) renders bare.
  const box = document.createElement('div');
  box.className = 'json-children';
  box.append(leaf(value as string | number | boolean | null));
  return box;
}

export function renderJson(el: HTMLElement, raw: string): void {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    const pre = document.createElement('pre');
    pre.className = 'json-raw';
    pre.textContent = raw;
    el.replaceChildren(pre);
    return;
  }
  el.replaceChildren(buildNode(value));
}
