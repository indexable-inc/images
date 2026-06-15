// Shared helpers for the namespace browser: the row shape the kernel publishes
// (`introspect.namespace_rows`), and the formatting both the inline body and the
// full rail view use, so there is one implementation of "how a variable reads".

// One variable (or, recursively, one member of a container). `children` is present
// only for expandable containers (dict/list/object), depth- and breadth-bounded by
// the producer; a single trailing `+N more` elision row may appear among them.
export interface NsRow {
  name: string;
  type: string;
  kind: string;
  repr: string;
  size: number;
  shape: string;
  children?: NsRow[];
  // Run ids that bound (`assigned_in`) or referenced (`used_in`) this variable,
  // most-recent-last, present only on top-level rows (provenance is a property of a
  // variable, not of a container's members). Each id is an exec pane's id, so the
  // view can link a name back to the runs behind it.
  assigned_in?: string[];
  used_in?: string[];
}

// Human byte size; empty for the sizeless (modules, functions report 0).
export function fmtSize(n: number): string {
  if (!n) return '';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

// The middle column: a frame/array describes itself by shape; everything else
// shows its repr, falling back to a shape (a container's length).
export function detail(row: NsRow): string {
  if (row.shape && (row.kind === 'frame' || row.kind === 'array')) return row.shape;
  return row.repr || row.shape;
}

// The three top-level groups the view buckets names into, in display order.
export type NsGroup = 'Data' | 'Modules' | 'Functions';
export const NS_GROUPS: readonly NsGroup[] = ['Data', 'Modules', 'Functions'];

// Which group a row belongs to: modules and functions/classes are shared machinery,
// everything else is the data the session holds.
export function groupOf(row: NsRow): NsGroup {
  if (row.kind === 'module') return 'Modules';
  if (row.kind === 'function' || row.kind === 'class') return 'Functions';
  return 'Data';
}

// Parse a namespace pane's JSON body into rows; malformed/absent → none.
export function parseRows(body: string | undefined): NsRow[] {
  try {
    const parsed = JSON.parse(body ?? '[]');
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

// The rendered namespace list, flattened to one item per visible line: either a
// group heading or a row at some indent depth. Building it once (and reading it
// for both rendering and keyboard navigation) guarantees that what `j`/`k` walk is
// exactly what the eye sees — group order, heaviest-first within a group, and only
// the children of expanded rows.
export type NsItem =
  | { kind: 'group'; name: NsGroup; count: number }
  | { kind: 'row'; path: string; row: NsRow; depth: number };

// A row's stable path: the caller's prefix, then `<group>_<index>` at the top
// level and an element index at each deeper level (`s0.1_4.2`). Paths are the key
// for selection and expansion and survive re-renders.
function pushRow(
  out: NsItem[],
  row: NsRow,
  path: string,
  depth: number,
  expanded: Record<string, boolean>,
): void {
  out.push({ kind: 'row', path, row, depth });
  if (row.children?.length && expanded[path]) {
    row.children.forEach((child, i) => pushRow(out, child, `${path}.${i}`, depth + 1, expanded));
  }
}

export function buildNsItems(
  rows: NsRow[],
  expanded: Record<string, boolean>,
  prefix: string,
): NsItem[] {
  const by: Record<NsGroup, NsRow[]> = { Data: [], Modules: [], Functions: [] };
  for (const row of rows) by[groupOf(row)].push(row);
  const out: NsItem[] = [];
  NS_GROUPS.forEach((name, gi) => {
    const group = by[name];
    if (!group.length) return;
    out.push({ kind: 'group', name, count: group.length });
    group.forEach((row, ri) => pushRow(out, row, `${prefix}.${gi}_${ri}`, 0, expanded));
  });
  return out;
}

// The path of a row's parent, or null for a top-level row (whose parent is the
// group, not a navigable row). A path keeps a parent only while the trimmed result
// still names a row (i.e. still contains a level separator).
export function nsParent(path: string): string | null {
  const i = path.lastIndexOf('.');
  if (i < 0) return null;
  const p = path.slice(0, i);
  return p.includes('.') ? p : null;
}
