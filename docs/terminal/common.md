# Terminal

PTY-backed terminal control for the repo: spawn interactive programs on real
pseudo-terminals, drive them like a human typing, read back a rendered screen,
multiplex one session across many clients, and emit terminal escape sequences.
The flagship is [tui](tui/overview.md), a PTY-driver library that runs gdb, vim,
a shell, or a REPL under a real PTY and exposes a VT-rendered viewport,
scrollback, and per-cell styling; [tui-node](tui-node/overview.md) and
[tui-py](tui-py/overview.md) are its Node and Python bindings. The VT engine
itself lives in [vt](vt/overview.md) (a safe Rust wrapper over ghostty's
libghostty-vt). [tap](tap/overview.md) is a separate, self-contained terminal
session manager (detach/attach/share). [terminal-theme](terminal-theme/overview.md),
[run](run/overview.md), and [kitty](kitty/overview.md) are small terminal
utilities used across the repo.

Read this page first, then the component page for the unit you are touching.

## Units

| unit | kind | role |
| --- | --- | --- |
| `packages/tui` | Rust lib crate (workspace member, no standalone flake output) | the PTY-driver: `TuiManager` spawns processes, each `TuiInstance` reads/writes one PTY through an actor backed by the [vt](vt/overview.md) engine. Optional `dashboard`/`publish` features re-export [dashboard-core](../dashboard/dashboard-core/overview.md). See [tui](tui/overview.md). |
| `packages/tui-node` | Rust `cdylib` + npm package (`nix build .#tui-node`, Linux only) | thin N-API binding over `tui`; npm `@indexable/tui`. See [tui-node](tui-node/overview.md). |
| `packages/tui-py` | Rust `cdylib` + wheel (`nix build .#tui-py`, Linux only) | PyO3 binding over `tui` plus a Python high-level API and a Playwright-style agent harness; PyPI `ix-tui`. See [tui-py](tui-py/overview.md). |
| `packages/tap` (`tap`, `tap-protocol`, `tap-pty`) | Rust workspace: `tap` binary (`nix run .#tap`), `tap-protocol`/`tap-pty` lib crates | terminal session manager for tiling-WM users: one daemon per session over a Unix socket, multiplayer attach, min-size negotiation. `tap-pty` is its multiplexable PTY engine (vt100 mirror + fan-out). See [tap](tap/overview.md). |
| `packages/vt` (`ix-vt`, `ix-vt-sys`, `libghostty-vt`) | `ix-vt`/`ix-vt-sys` Rust crates (`nix build .#ix-vt`); `libghostty-vt` Nix/Zig package (`nix build .#libghostty-vt`) | the VT engine: `ix-vt` is a safe wrapper, `ix-vt-sys` the raw FFI, `libghostty-vt` the ghostty C library it links. See [vt](vt/overview.md). |
| `packages/terminal-theme` | Rust lib crate (no standalone flake output) | detect light vs dark terminal background, gated on stdout being a TTY. See [terminal-theme](terminal-theme/overview.md). |
| `packages/run` | Nix-wrapped Python script (`nix run .#run`) | run a command under a recorded PTY session and keep the output (logs, asciinema cast, JSONL events). See [run](run/overview.md). |
| `packages/kitty` | Rust lib crate (no standalone flake output) | encoder for the kitty terminal graphics protocol (escape-sequence framing only). See [kitty](kitty/overview.md). |

The Rust crates above are all members of the repo's single Cargo workspace
(`lib/rust/workspace.nix` builds them through the shared cargo-unit graph). A
unit becomes a `nix run`/`nix build` flake output only when its `package.nix`
sets `flake`/`packageSet` (see [tap](tap/overview.md), [vt](vt/overview.md),
[run](run/overview.md), and the two binding packages); the plain library crates
(`tui`, `kitty`, `terminal-theme`, `ix-vt-sys`, `tap-pty`, `tap-protocol`) have
no flake output and are consumed only as dependencies.

## The PTY-driver model

Every "drive a real program" unit here follows the same shape:

```
caller --write--> PTY master --(kernel)--> PTY slave == child's controlling tty
child output --> PTY master --> VT engine (feed bytes) --> render snapshot
caller reads <-- viewport / scrollback / styled cells (rendered, not raw bytes)
```

1. Open a pseudo-terminal pair (master + slave) with
   [`pty-process`](https://docs.rs/pty-process). The child runs on the slave, so
   it sees a real terminal device: line discipline, job control, `SIGWINCH`, and
   terminfo all work, unlike a dumb pipe.
2. Read the child's output off the master and feed it into a VT emulator. The
   caller never parses escape sequences: it reads the rendered grid (a viewport
   of cells, scrollback above it, the cursor).
3. Write input to the master. A real terminal applies one input-side transform
   that the driver must reproduce: when the program enables DECCKM, arrow keys
   must be sent in application form (see glossary), or full-screen programs go
   blind to the arrows.

There are two VT engine choices in this domain, and they do not share code:
[tui](tui/overview.md) (and therefore both bindings) feeds the
[vt](vt/overview.md) / libghostty-vt engine; [tap](tap/overview.md)'s `tap-pty`
feeds the [`vt100`](https://docs.rs/vt100) crate. Both expose a viewport, a
cursor, and scrollback, but the types are independent.

## Cross-component invariants

- **Unix only.** PTY devices, `SIGWINCH`, and `setsid` are required, so
  everything here targets Linux and macOS, never Windows. The two binding wheels
  (`tui-node`, `tui-py`) additionally restrict their *Nix* output to Linux
  because the shared cdylib graph does not thread macOS's
  `-undefined dynamic_lookup` through to the link step (`packages/tui-py/package.nix:11`,
  `packages/tui-node/package.nix:8`); macOS dev uses a plain `cargo build`.
- **One actor owns each PTY.** In both `tui` and `tap-pty`, a single async task
  owns the PTY master and the child; every read, write, resize, kill, and the
  child reap funnel through one mailbox, so concurrent callers serialize without
  a lock on the fd. A child that has exited keeps its final screen readable;
  writes to it then error.
- **Default geometry is 80x24 with 10,000 lines of scrollback**
  (`packages/tui/src/types.rs:122-130`, `packages/tap/src/daemon.rs:32-34`).
  Resize delivers `SIGWINCH` to the child and updates the emulator together.
- **The xterm-256color contract.** `tui` spawns children with
  `TERM=xterm-256color` and `COLORTERM=truecolor` (`packages/tui/src/manager/spawn.rs:78-79`),
  because libghostty-vt implements an xterm-256color superset; without a fixed
  TERM, terminfo capabilities would vary by host.
- **Bindings are thin.** `tui-node` and `tui-py` add no terminal behavior: each
  holds a single process-wide `tui::TuiManager` in a `OnceLock`
  (`packages/tui-node/src/lib.rs:38`, `packages/tui-py/src/manager.rs:13`) and
  delegates every call to the `tui` crate, which owns the runtime and the engine
  thread.
- **The dashboard is out of this domain.** `tui`'s `dashboard`/`publish`
  features and both bindings expose a live web grid of terminals, but the HTTP
  server, the Loro CRDT document, the socket transport, and the wire types all
  live in [dashboard-core](../dashboard/dashboard-core/overview.md). This domain
  only adapts a live PTY manager into terminal panes; see the
  [dashboard domain](../dashboard/common.md) for everything downstream of that.

## Glossary

- **PTY (pseudo-terminal)**: a master/slave fd pair. The child runs on the slave
  (its controlling tty); the driver reads/writes the master.
- **VT engine / emulator**: the state machine that consumes a child's raw output
  (text plus escape sequences) and maintains a screen grid. Here it is either
  libghostty-vt (via [ix-vt](vt/overview.md)) or the `vt100` crate
  ([tap](tap/overview.md)).
- **viewport**: the visible screen, one row per line. **scrollback**: rows that
  have scrolled above the viewport, oldest first.
- **render snapshot**: an owned, point-in-time copy of the screen state
  (`ix_vt::Snapshot`): viewport cells, scrollback count, cursor. Valid after the
  terminal is written to again.
- **styled cell**: one screen cell with its character plus typed fg/bg color and
  bold/italic/underline/inverse flags (`tui::StyledCell`).
- **DECCKM / application cursor keys**: DEC private mode 1. A program enables it
  (terminfo `smkx`, as ncurses/vim/less do) to receive cursor keys as `ESC O A`
  rather than `ESC [ A`. The driver tracks the mode and rewrites arrows on
  write (`packages/tui/src/actor/mod.rs:238`).
- **alternate screen**: the full-screen buffer a TUI switches to (vim, less);
  exposed by `tap-pty` so a late attach can repaint it.
- **resync snapshot**: in `tap`, the escape sequences that reproduce the current
  screen, handed to a newly attached client so it paints correctly without
  replaying history (`packages/tap/pty/src/lib.rs:195`).
- **fan-out**: one child's output broadcast to many subscribers (tap multiplayer,
  tap `subscribe`).
- **producer / aggregator**: in the dashboard split, a process *publishes* its
  panes over a socket (producer); a standalone *aggregator* renders many. Both
  defined in the [dashboard domain](../dashboard/common.md).
- **kitty graphics protocol**: the `APC _G ... ST` escape framing terminals use
  to draw images; [kitty](kitty/overview.md) encodes it.

## Components

| component | page | what |
| --- | --- | --- |
| tui | [tui/overview.md](tui/overview.md) | PTY-driver library: manager, instance handle, VT-rendered reads, lifecycle; dashboard/publish wiring. Threading model in [internals](tui/internals.md). |
| tui-node | [tui-node/overview.md](tui-node/overview.md) | N-API bindings; `@indexable/tui` npm package |
| tui-py | [tui-py/overview.md](tui-py/overview.md) | PyO3 bindings, async Python API, agent harness; `ix-tui` wheel |
| tap | [tap/overview.md](tap/overview.md) | terminal session manager (daemon + clients) and the `tap-pty` multiplex engine + `tap-protocol` wire types |
| vt | [vt/overview.md](vt/overview.md) | `ix-vt` safe wrapper, `ix-vt-sys` FFI, `libghostty-vt` C library build |
| terminal-theme | [terminal-theme/overview.md](terminal-theme/overview.md) | light/dark background detection, TTY-gated |
| run | [run/overview.md](run/overview.md) | record a command's PTY session: logs, cast, JSONL events |
| kitty | [kitty/overview.md](kitty/overview.md) | kitty graphics protocol encoder |
