/**
 * Encode a keydown event into the byte string the PTY expects, or null when
 * the event is not ours to handle (bare modifiers, browser shortcuts, ...).
 */
export function encodeKey(ev: KeyboardEvent, appCursor: boolean): string | null {
  const key = ev.key;
  if (
    key === 'Shift' ||
    key === 'Control' ||
    key === 'Alt' ||
    key === 'Meta' ||
    key === 'CapsLock' ||
    key === 'Dead' ||
    key === 'Unidentified'
  ) {
    return null;
  }
  // Leave meta combos (cmd+c, cmd+t, ...) to the browser.
  if (ev.metaKey) {
    return null;
  }
  if (ev.ctrlKey) {
    if (key.length === 1) {
      const code = key.toUpperCase().charCodeAt(0);
      // @ A-Z [ \ ] ^ _ map onto C0 controls.
      if (code >= 64 && code <= 95) {
        return String.fromCharCode(code & 31);
      }
    }
    return null;
  }
  switch (key) {
    case 'Enter':
      return '\r';
    case 'Backspace':
      return '\x7f';
    case 'Tab':
      return '\t';
    case 'Escape':
      return '\x1b';
    case 'ArrowUp':
      return appCursor ? '\x1bOA' : '\x1b[A';
    case 'ArrowDown':
      return appCursor ? '\x1bOB' : '\x1b[B';
    case 'ArrowRight':
      return appCursor ? '\x1bOC' : '\x1b[C';
    case 'ArrowLeft':
      return appCursor ? '\x1bOD' : '\x1b[D';
    case 'Home':
      return '\x1b[H';
    case 'End':
      return '\x1b[F';
    case 'PageUp':
      return '\x1b[5~';
    case 'PageDown':
      return '\x1b[6~';
    case 'Delete':
      return '\x1b[3~';
    default:
      break;
  }
  if (key.length === 1) {
    return ev.altKey ? `\x1b${key}` : key;
  }
  return null;
}
