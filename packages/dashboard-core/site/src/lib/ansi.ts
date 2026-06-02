// Terminal-screen rendering: SGR parsing, the 256-color palette, and DOM
// building. Ported verbatim in behavior from the original vanilla dashboard so
// rendered output matches ghostty. Kept framework-agnostic; TermCard.svelte
// calls renderInto() from an effect.

interface Color {
  kind: 'idx' | 'rgb';
  value?: number;
  r?: number;
  g?: number;
  b?: number;
}

interface Style {
  fg: Color | null;
  bg: Color | null;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  inverse: boolean;
}

interface Run {
  text: string;
  style: Style;
}

export interface Cursor {
  row: number;
  col: number;
  shape: string;
}

// Strip CSI/SGR and other escape sequences, leaving plain text. For the exec
// renderer and the stream's one-line previews, where a captured byte stream is
// shown as text rather than the SGR-decoded grid renderInto() builds.
export function stripAnsi(text: string): string {
  // eslint-disable-next-line no-control-regex
  return text.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, '').replace(/\x1b[@-Z\\-_]/g, '');
}

// The 16 base ANSI colors mirror the user's ghostty themes
// (~/.config/nix/ghostty/themes/custom-{light,dark}). Colors 16-255 are the
// theme-independent xterm 6x6x6 cube and 24-step gray ramp the tui crate's SGR
// encoder targets. Rebuilt when the system flips light/dark.
const ANSI_16: Record<'light' | 'dark', number[][]> = {
  light: [
    [0, 0, 0], [219, 63, 57], [66, 147, 62], [133, 85, 4],
    [50, 94, 238], [147, 0, 147], [14, 112, 174], [143, 144, 150],
    [42, 44, 51], [219, 63, 57], [66, 147, 62], [133, 85, 4],
    [50, 94, 238], [147, 0, 147], [14, 112, 174], [255, 255, 255],
  ],
  dark: [
    [0, 0, 0], [243, 139, 168], [166, 227, 161], [249, 226, 175],
    [137, 180, 250], [203, 166, 247], [148, 226, 213], [224, 216, 192],
    [88, 91, 112], [243, 139, 168], [166, 227, 161], [249, 226, 175],
    [137, 180, 250], [203, 166, 247], [148, 226, 213], [250, 240, 200],
  ],
};

function buildPalette(base16: number[][]): number[][] {
  const table = base16.slice();
  const cube = [0, 95, 135, 175, 215, 255];
  for (let r = 0; r < 6; r++)
    for (let g = 0; g < 6; g++)
      for (let b = 0; b < 6; b++) table.push([cube[r], cube[g], cube[b]]);
  for (let i = 0; i < 24; i++) {
    const v = 8 + i * 10;
    table.push([v, v, v]);
  }
  return table;
}

const darkScheme = matchMedia('(prefers-color-scheme: dark)');
let PALETTE = buildPalette(darkScheme.matches ? ANSI_16.dark : ANSI_16.light);

// Subscribers repaint when the OS theme flips so chrome (CSS tokens) and
// terminal content (this palette) never disagree.
const themeListeners = new Set<() => void>();
darkScheme.addEventListener('change', () => {
  PALETTE = buildPalette(darkScheme.matches ? ANSI_16.dark : ANSI_16.light);
  for (const fn of themeListeners) fn();
});
export function onThemeChange(fn: () => void): () => void {
  themeListeners.add(fn);
  return () => themeListeners.delete(fn);
}

function cssColor(c: Color | null): string | null {
  if (!c) return null;
  if (c.kind === 'idx') {
    const rgb = PALETTE[c.value ?? 0] ?? [0, 0, 0];
    return `rgb(${rgb[0]},${rgb[1]},${rgb[2]})`;
  }
  if (c.kind === 'rgb') return `rgb(${c.r},${c.g},${c.b})`;
  return null;
}

function freshStyle(): Style {
  return { fg: null, bg: null, bold: false, italic: false, underline: false, inverse: false };
}

// Apply one SGR escape's numeric params in place. Mirrors the codes the tui
// crate emits: 0 reset, 1/3/4/7 attrs, 30-37/90-97 + 38;5/38;2 fg, 40-47/100-107
// + 48;5/48;2 bg.
function applySgr(style: Style, params: number[]): void {
  for (let i = 0; i < params.length; i++) {
    const p = params[i];
    if (p === 0) Object.assign(style, freshStyle());
    else if (p === 1) style.bold = true;
    else if (p === 3) style.italic = true;
    else if (p === 4) style.underline = true;
    else if (p === 7) style.inverse = true;
    else if (p === 22) style.bold = false;
    else if (p === 23) style.italic = false;
    else if (p === 24) style.underline = false;
    else if (p === 27) style.inverse = false;
    else if (p >= 30 && p <= 37) style.fg = { kind: 'idx', value: p - 30 };
    else if (p === 39) style.fg = null;
    else if (p >= 40 && p <= 47) style.bg = { kind: 'idx', value: p - 40 };
    else if (p === 49) style.bg = null;
    else if (p >= 90 && p <= 97) style.fg = { kind: 'idx', value: p - 90 + 8 };
    else if (p >= 100 && p <= 107) style.bg = { kind: 'idx', value: p - 100 + 8 };
    else if (p === 38 || p === 48) {
      const target = p === 38 ? 'fg' : 'bg';
      if (params[i + 1] === 5) { style[target] = { kind: 'idx', value: params[i + 2] }; i += 2; }
      else if (params[i + 1] === 2) {
        style[target] = { kind: 'rgb', r: params[i + 2], g: params[i + 3], b: params[i + 4] };
        i += 4;
      }
    }
  }
}

// Split one SGR-coded row into runs sharing one style; escapes are consumed.
function parseRow(line: string): Run[] {
  const runs: Run[] = [];
  const style = freshStyle();
  let text = '';
  let i = 0;
  while (i < line.length) {
    const esc = line.indexOf('\x1b[', i);
    if (esc === -1) { text += line.slice(i); break; }
    text += line.slice(i, esc);
    const end = line.indexOf('m', esc);
    if (end === -1) { text += line.slice(esc); break; }
    if (text) { runs.push({ text, style: { ...style } }); text = ''; }
    const body = line.slice(esc + 2, end);
    const params = body === '' ? [0] : body.split(';').map((s) => parseInt(s, 10) || 0);
    applySgr(style, params);
    i = end + 1;
  }
  if (text) runs.push({ text, style: { ...style } });
  return runs;
}

function applyStyleCss(span: HTMLSpanElement, style: Style): void {
  let fg = cssColor(style.fg);
  let bg = cssColor(style.bg);
  if (style.inverse) [fg, bg] = [bg ?? 'var(--panel)', fg ?? 'var(--ink)'];
  if (fg) span.style.color = fg;
  if (bg) span.style.background = bg;
  let cls = '';
  if (style.bold) cls += ' b';
  if (style.italic) cls += ' i';
  if (style.underline) cls += ' u';
  if (cls) span.className = cls.trim();
}

function styledSpan(text: string, style: Style): HTMLSpanElement {
  const span = document.createElement('span');
  span.textContent = text;
  applyStyleCss(span, style);
  return span;
}

function cursorSpan(ch: string, style: Style, shape: string): HTMLSpanElement {
  const span = styledSpan(ch || ' ', style);
  const kind = shape === 'bar' ? 'bar' : shape === 'underline' ? 'underline' : 'block';
  span.classList.add('cur', kind);
  return span;
}

// True when the screen has visible text once SGR is stripped, or any styled
// cell (a colored bar carries no plain text but is not empty).
export function hasOutput(screen: string): boolean {
  const stripped = screen.replace(/[\x1b][[][0-9;]*m/g, '');
  return stripped.trim() !== '' || stripped.length !== screen.length;
}

// Rebuild `el`'s children to render `screen` with an optional cursor. Clears
// first. The cursor cell is split out so its overlay lands on one character; a
// cursor past the row end gets a synthetic space.
export function renderInto(el: HTMLElement, screen: string, cursor: Cursor | null): void {
  const frag = document.createDocumentFragment();
  const lines = screen.split('\n');
  lines.forEach((line, row) => {
    if (row > 0) frag.append(document.createTextNode('\n'));
    const runs = parseRow(line);
    const cursorCol = cursor && cursor.row === row ? cursor.col : -1;
    let col = 0;
    for (const run of runs) {
      let start = 0;
      for (let k = 0; k < run.text.length; k++) {
        if (col + k === cursorCol) {
          if (k > start) frag.append(styledSpan(run.text.slice(start, k), run.style));
          frag.append(cursorSpan(run.text[k], run.style, cursor!.shape));
          start = k + 1;
        }
      }
      if (start < run.text.length) frag.append(styledSpan(run.text.slice(start), run.style));
      col += run.text.length;
    }
    if (cursorCol >= col) {
      if (cursorCol > col) frag.append(document.createTextNode(' '.repeat(cursorCol - col)));
      frag.append(cursorSpan(' ', freshStyle(), cursor!.shape));
    }
  });
  el.replaceChildren(frag);
}

// Width in px of one monospace cell at font-size 1px, measured against the same
// font stack the cards use. Monospace advance scales linearly with size, so one
// measurement converts cols <-> px at any size. Remeasured on fonts.ready.
let cellRatio = 0.6;
let measureCanvas: HTMLCanvasElement | null = null;
export function measureCellRatio(): number {
  const probe = getComputedStyle(document.documentElement).getPropertyValue('--mono').trim();
  measureCanvas ??= document.createElement('canvas');
  const ctx = measureCanvas.getContext('2d');
  if (!ctx) return cellRatio;
  ctx.font = `100px ${probe}`;
  const w = ctx.measureText('MMMMMMMMMM').width / 10;
  if (w > 0) cellRatio = w / 100;
  return cellRatio;
}
export function getCellRatio(): number {
  return cellRatio;
}
