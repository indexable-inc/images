# tap

`packages/tap` is a terminal session manager for tiling-WM users: start a
command, detach, reattach later from any terminal, and share a session with
others, with no in-terminal tiling layer to fight a window manager. It is a
self-contained Cargo workspace of three crates, independent of
[tui](../tui/overview.md) (it mirrors the same PTY-actor pattern but uses the
[`vt100`](https://docs.rs/vt100) emulator, not the [vt](../vt/overview.md)
engine). The `tap` binary is the flake output `nix run .#tap`
(`package.nix:3-4`).

## The three crates

| crate | role |
| --- | --- |
| `packages/tap` (`tap`) | the CLI, the session daemon, and the attach client (`src/`). Binary `tap`. |
| `packages/tap/pty` (`tap-pty`) | the reusable multiplexable PTY session engine: spawn a child on a PTY, mirror it with vt100, fan raw output out to many subscribers with atomic resync snapshots. Library, no flake output. |
| `packages/tap/protocol` (`tap-protocol`) | wire types and runtime paths, no I/O. Library, no flake output. |

## Architecture: one daemon per session, thin clients

Every interactive `tap` (even for a single user) is a client of a per-session
daemon, connected over a Unix socket speaking newline-delimited JSON. The daemon
owns the PTY and the screen; clients only render it and forward keystrokes. That
uniformity is what makes resize-while-attached and multi-client sharing fall out
(`src/daemon.rs:1-13`).

```
tap start <cmd>  --spawns--> daemon (setsid, owns PtySession + client registry)
                                |  UnixListener at <runtime>/<id>.sock
tap attach [id] --connects--> client (raw-mode terminal: render output, send input)
tap subscribe   --connects--> read-only observer (output stream only)
```

`tap start` resolves the command (default interactive `$SHELL`, forced
`-i`/`-l`), then spawns the daemon as a detached child (`stdin/out` to
`/dev/null`, `stderr` to a per-session log) and polls for its socket to appear
before attaching (`src/client.rs:32-104`). The daemon spawns the PTY child first,
then calls `setsid` so closing the launching client does not SIGHUP it; the order
matters because macOS rejects the child's `TIOCSCTTY` if the parent is already a
session leader (`src/daemon.rs:250-256`).

## CLI (`src/main.rs`)

`tap` with no subcommand defaults to `start` (`main.rs:114`). Subcommands
(`main.rs:35`):

- `start [-d|--detached] [--id <id>] [cmd...]`: start a session; `-d` does not
  attach.
- `attach [session]`: attach to a session (most recent if omitted).
- `list`: table of live sessions.
- `scrollback [-s id] [-l lines]`, `cursor [-s id]`, `size [-s id]`: one-shot
  reads of the screen text, cursor, negotiated size.
- `inject [-s id] <text>`: type text into a session without attaching.
- `subscribe [-s id]`: stream raw output to stdout.
- `kill [session]`: terminate the child and shut the daemon down.
- `daemon --id --socket -- <cmd...>`: hidden, spawned by `start`; not for direct
  use (`main.rs:96`).

Interactive keybinds (configurable): `Ctrl-\` detaches, `Alt-e` opens the
scrollback in `$EDITOR` (`README.md:36`).

## tap-pty: the session engine (`pty/src/lib.rs`)

`PtySession::spawn(SessionConfig { command, rows, cols, scrollback_lines })`
(`pty/src/lib.rs:136`) runs the child on a `pty-process` PTY and mirrors output
into a `vt100::Parser` behind a `parking_lot::Mutex`. An actor task owns the PTY
master and is the only writer (`pty/src/lib.rs:307`); a `Command` channel
serializes `Write`/`ResizePty`/`Kill`. Output is broadcast on a
`tokio::sync::broadcast` channel of capacity 1024.

The load-bearing property is **atomic late attach** (`pty/src/lib.rs:9-13,
190-199`): `subscribe() -> Attachment { snapshot, output }` takes the emulator
lock, reads `screen().contents_formatted()` (the escape sequences that reproduce
the current screen), and subscribes to the broadcast under that same lock. The
actor also holds the lock across `process(bytes)` + `send(bytes)`
(`pty/src/lib.rs:345-348`), so the snapshot/stream join is exactly once: no byte
is both in the snapshot and the first stream item, and none is dropped between.
That is what makes attaching to a running full-screen TUI paint correctly.

Other handle methods: `snapshot()`, `write_input`, `resize` (updates the
emulator immediately, queues the kernel PTY resize that delivers `SIGWINCH`),
`kill`, `scrollback(lines)`, `cursor`, `size`, `alternate_screen`, `exit_watch`
(a `watch::Receiver<Option<i32>>`), `exit_code`. Exit codes resolve to
`128 + signal` when signalled (`pty/src/lib.rs:294`). A lagged subscriber resyncs
from a fresh snapshot rather than a torn byte replay (`src/daemon.rs:408`).

## Size negotiation (multiplayer)

The daemon keeps a client registry (`src/daemon.rs:55-59`). The session size is
the element-wise minimum over all attached clients (tmux's rule), so no client
sees clipped output (`recompute`, `src/daemon.rs:189`). On any change it resizes
the PTY and pushes a fresh repaint (`Response::Resized`) to every attached client
except the requester. Observers from `subscribe` do not contribute to size
negotiation (`add_observer`, `src/daemon.rs:136`). A client whose own terminal is
larger than the negotiated size gets a dim "unused space" warning row.

## tap-protocol: the wire (`protocol/src/lib.rs`)

A client and daemon exchange one JSON object per line; binary payloads (raw PTY
bytes, resync snapshots) ride as base64 so a frame never contains a literal
newline (`protocol/src/lib.rs:178`). `Request` variants (`protocol/src/lib.rs:39`):
`Attach{rows,cols}`, `Detach`, `Input{data}`, `Resize{rows,cols}`, `Inject{data}`,
`Subscribe`, `GetScrollback{lines}`, `GetCursor`, `GetSize`, `Kill`. `Response`
variants (`:86`): `Attached{rows,cols,snapshot}`, `Output{data}`,
`Resized{rows,cols,snapshot}`, `Scrollback`, `Cursor`, `Size`, `Subscribed`,
`SessionEnded{exit_code}`, `Ok`, `Error{message}`. `Session` (`:23`) is the index
record (id, daemon pid, start time, command, socket path); liveness is resolved
from pid/socket, not trusted from the record.

Runtime paths (`protocol/src/lib.rs:156-176`): `runtime_dir()` honors
`TAP_RUNTIME_DIR` (`RUNTIME_DIR_ENV`), else the XDG runtime dir, else `~/.tap`,
else a temp dir; `socket_path(id)` is `<dir>/<id>.sock`; `sessions_file()` is
`<dir>/sessions.json`.

## Configuration

Optional `~/.config/tap/config.toml` (`src/config.rs`): `editor` (overrides
`$EDITOR`/`$VISUAL`), `[keybinds] editor`/`detach`, `[timing] escape_timeout_ms`
(distinguishing a lone `ESC` from an `Alt-` sequence, default 50). Keybinds match
both legacy byte sequences and the Kitty keyboard protocol's CSI-u form
(`config.rs:135-179`), so a bind fires whether or not an inner app negotiated
Kitty input.

## Build and tests

`default.nix` selects the `tap` binary from the shared workspace graph
(`ix.cargoUnit.selectBinaryWithTests`). `tap`, `tap-pty`, and `tap-protocol` all
set `passthruTests = true`. Integration tests (`tests/session.rs`) drive the real
`tap` binary on a PTY using this repo's [tui](../tui/overview.md) driver
(`Cargo.toml:27`) and assert on the rendered grid: input round-trip, full-screen
resync on a second attach, multiplayer min-size, and resize-while-attached.

## Known limits

Unix only (PTYs, `SIGWINCH`, `setsid`). Sessions are local to one machine and
user (the socket is in the user's runtime dir); no network or cross-user access.
A client that falls far behind is resynced from a snapshot, so a burst can skip
intermediate frames on a slow link (`README.md:110-117`).
