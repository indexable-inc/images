/**
 * Node.js bindings for the `tui` PTY-backed terminal manager.
 *
 * Spawn child processes attached to real pseudo-terminals, drive them with
 * keystrokes, and read back a VT100-rendered viewport. Every I/O method returns
 * a `Promise` and runs on the tui actor, so it never blocks the event loop.
 */

/** Spawn-time terminal configuration. Unset fields use 80x24 / 10,000 lines. */
export interface SpawnOptions {
  rows?: number;
  cols?: number;
  scrollbackLines?: number;
}

/** Scrollback history plus the visible viewport, read together. */
export interface FullOutput {
  scrollback: Array<string>;
  viewport: Array<string>;
}

/** A single spawned PTY-backed process. */
export declare class Tui {
  /** Spawn `command` on a fresh PTY and start tracking it. */
  constructor(command: string, args?: Array<string>, options?: SpawnOptions);

  /** Every instance the process is currently tracking. */
  static listAll(): Array<Tui>;

  readonly id: string;
  readonly command: string;
  readonly args: Array<string>;
  /** Current terminal height in rows. */
  readonly rows: number;
  /** Current terminal width in columns. */
  readonly cols: number;
  readonly scrollbackLimit: number;

  /** Send `data` to the PTY exactly as given. */
  write(data: string): Promise<void>;
  /** The current viewport, one string per visible row. */
  readViewport(): Promise<Array<string>>;
  /** Lines that have scrolled above the viewport, oldest first. */
  readScrollback(): Promise<Array<string>>;
  /** Scrollback and viewport read together. */
  readFull(): Promise<FullOutput>;
  /** Read the viewport, waiting up to `timeoutMs` for first content. */
  readBlocking(timeoutMs: number): Promise<Array<string>>;

  /** Whether the child process is still running. */
  isAlive(): boolean;
  /** The exit code, or `null` while running or if terminated by a signal. */
  exitCode(): number | null;
  /** Resolve once the child exits, returning its exit code (`null` if signaled). */
  wait(): Promise<number | null>;
  /** Force-terminate the child with SIGKILL. A no-op if already exited. */
  kill(): Promise<void>;
  /** Resize the terminal (delivers SIGWINCH to the child). */
  resize(rows: number, cols: number): Promise<void>;
  /** Force-kill the child and stop tracking it. */
  close(): void;
}

/** Handle to a running web dashboard. */
export declare class Dashboard {
  /** The URL to open in a browser. */
  readonly url: string;
  /** The bound `host:port`. */
  readonly addr: string;
  /** Stop the server and its poll loop. Idempotent. */
  stop(): void;
}

/**
 * Start the Loro-backed web dashboard for every live terminal in this process.
 * `host` must be an IP literal; pass `port = 0` for an ephemeral port.
 */
export declare function serve(host?: string, port?: number, pollMs?: number): Dashboard;

/** A pattern for {@link waitFor}: substring, RegExp, or viewport predicate. */
export type WaitPattern = string | RegExp | ((viewport: Array<string>) => boolean);

/** Common keystrokes as ANSI byte sequences, plus `ctrl`/`alt` helpers. */
export declare const Key: {
  readonly ENTER: string;
  readonly TAB: string;
  readonly ESC: string;
  readonly BACKSPACE: string;
  readonly DELETE: string;
  readonly UP: string;
  readonly DOWN: string;
  readonly RIGHT: string;
  readonly LEFT: string;
  readonly HOME: string;
  readonly END: string;
  readonly PAGE_UP: string;
  readonly PAGE_DOWN: string;
  readonly CTRL_C: string;
  readonly CTRL_D: string;
  ctrl(letter: string): string;
  alt(letter: string): string;
};

/** Poll a terminal's viewport until `pattern` matches; returns the lines. */
export declare function waitFor(
  tui: Tui,
  pattern: WaitPattern,
  options?: { timeoutMs?: number; pollMs?: number },
): Promise<Array<string>>;
