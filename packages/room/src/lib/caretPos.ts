// Pixel position of a textarea character offset.
//
// HTML textareas have no native API for this — we mirror the textarea
// into an off-screen <div> that inherits the same typography, content,
// and width, drop a marker <span> at the offset, then read the span's
// offsetTop/offsetLeft. Equivalent to component/textarea-caret-position
// but stripped to the props that actually matter for our composer.
//
// The mirror div is reused across calls and lives at document.body so
// it picks up the page's font-loading and zoom state. Hidden via
// visibility:hidden (NOT display:none) so layout still runs.

const MIRROR_ID = '__room_caret_mirror__';

const STYLE_PROPS = [
  'boxSizing',
  'width',
  'fontFamily',
  'fontSize',
  'fontStyle',
  'fontVariant',
  'fontWeight',
  'fontStretch',
  'lineHeight',
  'letterSpacing',
  'wordSpacing',
  'tabSize',
  'textIndent',
  'textTransform',
  'textRendering',
  'paddingTop',
  'paddingRight',
  'paddingBottom',
  'paddingLeft',
  'borderTopWidth',
  'borderRightWidth',
  'borderBottomWidth',
  'borderLeftWidth',
  'borderStyle',
  'whiteSpace',
  'overflowWrap',
  'wordWrap',
  'wordBreak',
  'direction'
] as const;

function getMirror(): HTMLDivElement {
  let el = document.getElementById(MIRROR_ID) as HTMLDivElement | null;
  if (el) return el;
  el = document.createElement('div');
  el.id = MIRROR_ID;
  el.style.position = 'absolute';
  el.style.top = '0';
  el.style.left = '0';
  el.style.visibility = 'hidden';
  el.style.overflow = 'hidden';
  el.style.whiteSpace = 'pre-wrap';
  el.style.wordWrap = 'break-word';
  document.body.appendChild(el);
  return el;
}

export interface CaretCoords {
  /** Offset from textarea top-left in pixels. Already accounts for
   *  the textarea's own scrollTop. */
  top: number;
  left: number;
  /** Computed line-height in pixels (height of the caret bar). */
  height: number;
}

export function getCaretCoords(
  textarea: HTMLTextAreaElement,
  position: number,
  value: string = textarea.value
): CaretCoords {
  const mirror = getMirror();
  const style = window.getComputedStyle(textarea);
  for (const prop of STYLE_PROPS) {
    mirror.style[prop] = style[prop];
  }

  // Substring up to position becomes the "before-caret" text. Use a
  // visible marker for the caret itself so its bounding box is
  // measurable. If position lands on a trailing newline we'd
  // otherwise collapse height; the trailing "." enforces a line box.
  // Callers pass `value` explicitly when the source-of-truth string
  // is ahead of the textarea's current DOM value (e.g. mid-Svelte-
  // reactivity-flush) so the caret doesn't lag behind the text.
  const before = value.substring(0, position);
  const after = value.substring(position) || '.';

  mirror.textContent = before;
  const span = document.createElement('span');
  span.textContent = after;
  mirror.appendChild(span);

  const top = span.offsetTop - textarea.scrollTop;
  const left = span.offsetLeft - textarea.scrollLeft;
  const height = parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.2;

  // Reset content so the mirror stays small and doesn't leak nodes.
  mirror.textContent = '';

  return { top, left, height };
}
