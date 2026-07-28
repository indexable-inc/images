"use strict";

// Thin JS layer over the native N-API addon. The addon (the `tui` Rust crate)
// owns all PTY behavior; this file only adds keystroke constants and a
// poll-based `waitFor`, mirroring the Python package's ergonomics.

const native = require("./native/tui_node.node");

const { Tui, Dashboard, serve } = native;

/** Common keystrokes as ANSI byte sequences, plus `ctrl`/`alt` helpers. */
const Key = Object.freeze({
  ENTER: "\r",
  TAB: "\t",
  ESC: "\x1b",
  BACKSPACE: "\x7f",
  DELETE: "\x1b[3~",
  UP: "\x1b[A",
  DOWN: "\x1b[B",
  RIGHT: "\x1b[C",
  LEFT: "\x1b[D",
  HOME: "\x1b[H",
  END: "\x1b[F",
  PAGE_UP: "\x1b[5~",
  PAGE_DOWN: "\x1b[6~",
  CTRL_C: "\x03",
  CTRL_D: "\x04",
  /** Ctrl+<letter> as a single control byte. */
  ctrl(letter) {
    const ch = String(letter).toLowerCase();
    if (ch.length !== 1 || ch < "a" || ch > "z") {
      throw new RangeError(`Key.ctrl expects one ASCII letter a-z, got ${letter}`);
    }
    return String.fromCharCode(ch.charCodeAt(0) - 96);
  },
  /** Alt+<letter> as ESC followed by the character. */
  alt(letter) {
    if (String(letter).length !== 1) {
      throw new RangeError(`Key.alt expects a single character, got ${letter}`);
    }
    return "\x1b" + letter;
  },
});

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Poll a terminal's viewport until it matches, returning the matching lines.
 * `pattern` is a substring, a RegExp, or a predicate over the viewport lines.
 */
async function waitFor(tui, pattern, { timeoutMs = 5000, pollMs = 50 } = {}) {
  const matches =
    typeof pattern === "function"
      ? pattern
      : pattern instanceof RegExp
        ? (lines) => pattern.test(lines.join("\n"))
        : (lines) => lines.join("\n").includes(String(pattern));

  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const lines = await tui.readViewport();
    if (matches(lines)) return lines;
    if (Date.now() >= deadline) {
      throw new Error(`waitFor: ${tui.command} did not match within ${timeoutMs}ms`);
    }
    await sleep(pollMs);
  }
}

// Assign each binding as its own property so Node's cjs-module-lexer detects
// them as named exports; `import { Tui, Key, waitFor }` then works from ESM.
exports.Tui = Tui;
exports.Dashboard = Dashboard;
exports.serve = serve;
exports.Key = Key;
exports.waitFor = waitFor;
