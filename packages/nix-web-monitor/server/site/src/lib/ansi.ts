/**
 * Minimal SGR (Select Graphic Rendition) parser. Recognises the subset of
 * codes Nix actually emits via `--log-format internal-json`: reset, bold,
 * 8-color foreground, bright foreground, and the matching backgrounds.
 *
 * Each segment carries the styling that was active when its text was emitted.
 * Render with one `<span>` per segment; the consumer maps `fg`/`bg`/`bold` to
 * CSS classes.
 */

export type AnsiColor =
  | 'black'
  | 'red'
  | 'green'
  | 'yellow'
  | 'blue'
  | 'magenta'
  | 'cyan'
  | 'white'
  | 'bright-black'
  | 'bright-red'
  | 'bright-green'
  | 'bright-yellow'
  | 'bright-blue'
  | 'bright-magenta'
  | 'bright-cyan'
  | 'bright-white';

export type AnsiSegment = Readonly<{
  text: string;
  fg: AnsiColor | null;
  bg: AnsiColor | null;
  bold: boolean;
}>;

const BASE_COLORS = [
  'black',
  'red',
  'green',
  'yellow',
  'blue',
  'magenta',
  'cyan',
  'white'
] as const satisfies ReadonlyArray<AnsiColor>;

const ESC = '\x1b';

type Style = {
  fg: AnsiColor | null;
  bg: AnsiColor | null;
  bold: boolean;
};

const EMPTY_STYLE: Style = { fg: null, bg: null, bold: false };

export function parseAnsi(text: string): ReadonlyArray<AnsiSegment> {
  const segments: AnsiSegment[] = [];
  const style: Style = { ...EMPTY_STYLE };
  let buffer = '';

  const flush = () => {
    if (buffer.length === 0) return;
    segments.push({ text: buffer, fg: style.fg, bg: style.bg, bold: style.bold });
    buffer = '';
  };

  let i = 0;
  while (i < text.length) {
    if (text[i] === ESC && text[i + 1] === '[') {
      const end = text.indexOf('m', i + 2);
      if (end === -1) {
        // Unterminated CSI; treat the rest as plain text.
        buffer += text.slice(i);
        break;
      }
      flush();
      applyCodes(style, text.slice(i + 2, end));
      i = end + 1;
      continue;
    }
    buffer += text[i];
    i += 1;
  }

  flush();
  return segments;
}

function applyCodes(style: Style, params: string): void {
  const codes = params.length === 0 ? [0] : params.split(';').map((part) => Number(part));
  for (const code of codes) {
    if (!Number.isFinite(code) || code === 0) {
      style.fg = null;
      style.bg = null;
      style.bold = false;
      continue;
    }
    if (code === 1) {
      style.bold = true;
    } else if (code === 22) {
      style.bold = false;
    } else if (code === 39) {
      style.fg = null;
    } else if (code === 49) {
      style.bg = null;
    } else if (code >= 30 && code <= 37) {
      style.fg = BASE_COLORS[code - 30];
    } else if (code >= 40 && code <= 47) {
      style.bg = BASE_COLORS[code - 40];
    } else if (code >= 90 && code <= 97) {
      style.fg = `bright-${BASE_COLORS[code - 90]}`;
    } else if (code >= 100 && code <= 107) {
      style.bg = `bright-${BASE_COLORS[code - 100]}`;
    }
    // 256-color (38;5;n) and truecolor (38;2;r;g;b) skipped: Nix doesn't
    // emit these, and adding them would force per-segment inline styles.
  }
}
