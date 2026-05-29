# tap

A terminal session manager for tiling-WM users. Start a command, detach from it,
and reattach later from any terminal, with no in-terminal tiling layer to fight
your window manager. Multiple people can attach to one session at once.

```sh
nix run .#tap                 # start a session in the current terminal
nix run .#tap -- attach       # reattach to the most recent session
```

## Why not tmux

If you use a tiling window manager (i3, Sway, Aerospace, yabai) you already tile
windows. tmux then runs a second tiling system inside the terminal: competing
keybinds, nested navigation, panes that duplicate what your WM windows already
do. tap keeps the one feature you actually came for, session persistence, and
drops the rest. Your WM tiles; tap persists.

## Commands

```sh
tap                       # start a session (interactive $SHELL)
tap start <cmd...>        # start a session running a command
tap start -d <cmd...>     # start in the background, do not attach
tap attach [id]           # attach to a session (most recent if omitted)
tap list                  # list live sessions
tap kill [id]             # stop a session and its child
tap scrollback [-s id]    # print the session's screen as text
tap cursor [-s id]        # print the cursor position
tap size [-s id]          # print the negotiated size
tap inject [-s id] <text> # type text into a session without attaching
tap subscribe [-s id]     # stream the session's raw output
```

Detach from an attached session with `Ctrl-\`. Open the scrollback in `$EDITOR`
with `Alt-e`. Both keybinds are configurable (see Configuration).

## Multiplayer

Several clients can attach to the same session and see the same screen live.
Input from any client reaches the shared child, so two people can drive one
terminal. The session is sized to the **smallest** attached client (the
element-wise minimum of every client's rows and columns), the same rule tmux
uses, so no client ever sees clipped output. A client whose own terminal is
larger than the negotiated size gets a dim warning on its bottom row noting that
the extra space is unused.

```sh
# terminal A
tap start --id pair bash
# terminal B (anywhere on the same machine)
tap attach pair
```

## Running it

tap is a package in this repository's Cargo workspace, exposed as a flake app:

```sh
nix run .#tap -- <args>     # run
nix build .#tap             # build the binary
```

## Architecture

tap is one daemon per session plus any number of thin clients, connected over a
per-session Unix socket speaking newline-delimited JSON (binary payloads ride as
base64). Every interactive `tap`, even for a single user, is a client of a
daemon. That uniformity is what makes resize-while-attached and multi-client
sharing work: the daemon owns the PTY and the screen, and clients only render it
and forward keystrokes.

The code is three composable crates:

- [`tap-pty`](pty) is the reusable session engine: it spawns a child on a real
  PTY (via [`pty-process`](https://docs.rs/pty-process)), mirrors it in a
  [`vt100`](https://docs.rs/vt100) emulator, and fans the raw output out to many
  subscribers. A new subscriber gets an atomic resync snapshot (the escape
  sequences that reproduce the current screen) joined to the live stream with no
  gap and no duplicated byte, which is what makes attaching to a running
  full-screen TUI paint correctly.
- [`tap-protocol`](protocol) is the wire types and runtime paths, with no I/O.
- `tap` (this crate) is the CLI, the daemon, and the attach client.

Integration tests in [`tests/integration.rs`](tests/integration.rs) drive the
real `tap` binary on a PTY using this repository's [`tui`](../tui) driver and
assert on the rendered grid: round-trip input, full-screen resync on a second
attach, multiplayer min-size negotiation, and resize-while-attached.

## Configuration

Optional, at `~/.config/tap/config.toml`:

```toml
editor = "nvim"            # overrides $EDITOR / $VISUAL for the Alt-e keybind

[keybinds]
editor = "Alt-e"
detach = "Ctrl-\\"

[timing]
escape_timeout_ms = 50     # window to tell a lone ESC from an Alt- sequence
```

Session sockets and the session index live under `$XDG_RUNTIME_DIR/tap` (falling
back to `~/.tap`). Set `TAP_RUNTIME_DIR` to relocate them, for example to isolate
test state.

## Known limitations

- Unix only (Linux and macOS). It relies on PTYs, `SIGWINCH`, and `setsid`.
- Sessions are local to one machine and one user; there is no network transport
  or cross-user access. The socket lives in the user's runtime directory.
- A client that falls far behind the output stream is resynced from a fresh
  snapshot rather than a byte-exact replay, so a burst can skip intermediate
  frames on a slow link.
- The `Alt-e` editor keybind is detected with a short escape timeout; on a slow
  or split input read a literal `Alt-e` can be delayed by `escape_timeout_ms`.
