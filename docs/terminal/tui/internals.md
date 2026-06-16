# tui internals

The non-obvious mechanism behind [tui](overview.md): a two-tier per-child
threading model, the cursor-key rewrite on write, the first-paint wait, and the
scrollback read. Read [overview](overview.md) first for the public surface.

## Two tiers per child: actor task + engine thread

Each spawned child gets two owners (`src/manager/spawn.rs:96-119`):

1. A **PTY actor**, an async task on the manager's runtime (`actor::pty_actor`,
   `src/actor/mod.rs:59`). It owns the PTY master and the `tokio::process::Child`
   and is the only thing that touches them. A `select!` loop services the command
   mailbox (`mpsc::Receiver<PtyCommand>`), reads PTY output into an 8 KiB buffer,
   and reaps the child. Because the actor is single, reads/writes/kill/resize
   from many threads serialize through one mailbox instead of locking the fd.
2. A **VT engine thread**, a dedicated OS thread (`actor::engine::spawn`,
   `src/actor/engine.rs:62`). It owns the `ix_vt::Terminal`. This split exists
   because libghostty-vt's terminal is `!Send + !Sync` (it has thread affinity,
   `src/lib.rs` doc and `packages/vt/ix-vt/src/lib.rs:289`): it cannot live in a
   tokio task that may migrate worker threads. The actor forwards every byte feed
   and read request to the engine thread as an `EngineRequest`
   (`engine.rs:25`); replies ride back on per-request `oneshot` channels, so the
   async side never touches the terminal.

```
caller (any thread)
  -> TuiInstance method -> runtime.block_on / await
  -> PtyCommand on mpsc  -> PTY actor task (owns PTY master + Child)
                              -> EngineRequest on std::mpsc -> VT engine thread (owns !Send Terminal)
                              <- oneshot reply (Snapshot / lines / ())
```

Engine init is a handshake: `Terminal::new` runs on the new thread, and `spawn`
blocks on a sync channel so a failed construction surfaces as `Error::VtEngine`
instead of a channel into a dead thread (`engine.rs:91-96`).

`PtyCommand` variants (`src/actor/mod.rs:17`): `Write`, `Kill`, `Resize`, and the
five reads (`ReadViewport`, `ReadScrollback`, `ReadChars`, `ReadStyledCells`,
`ReadCursor`). `EngineRequest` variants (`engine.rs:25`): `Process` (fire-and-
forget byte feed), `Resize`, `Snapshot`, `Scrollback`.

The actor's `select!` is `biased` (`actor/mod.rs:74`): commands first, then PTY
reads, then the child-exit reap. Child reap publishes the exit code through a
`watch::Sender<ExitState>` that `exit_state`/`is_alive`/`wait` read. After the
child exits, the actor keeps serving reads (the final screen stays inspectable)
and stays alive until every handle drops; a write then returns `TuiNotFound`
(`actor/mod.rs:92`).

## Cursor-key rewrite (DECCKM)

A real terminal emits cursor keys in application form (`ESC O A`..`D`,
Home/End as `ESC O H`/`F`) once a program enables DECCKM via terminfo `smkx`
(ncurses, vim, less all do on entry). Sending the normal `ESC [ A` form instead
leaves those programs blind to the arrows. So on every `Write`, the actor calls
`apply_cursor_key_mode` (`src/actor/mod.rs:238`): when application mode is on, it
rewrites the exact 3-byte `ESC [ {A,B,C,D,H,F}` sequences to their `ESC O` form
and passes everything else through. A modified arrow carries parameters
(`ESC [ 1 ; 5 A` for Ctrl+Up), so the byte after `[` is a digit, not a final
letter, and it is left untouched. The rewrite is per-write, not across writes: an
arrow split across two `write` calls is not reassembled.

The mode itself is read from the engine. After each `Process`, the engine thread
queries `terminal.application_cursor_keys()` and updates a shared
`Arc<RwLock<bool>>` (`engine.rs:156`), which the actor reads on the next write.
A failed query keeps the last known value.

## Cursor shape cache

`CursorShape` is updated on every render: the engine writes
`CursorShape::from(snapshot.cursor.visual_style)` into a shared
`Arc<RwLock<CursorShape>>` (`engine.rs:169`), so `TuiInstance::cursor_shape()`
reads it synchronously without a round trip. `size` is shared the same way so a
`resize` on one handle is visible from every clone (`manager/mod.rs:39`).

## First-paint wait

`spawn_tui` ends with `wait_for_initial_output` (`src/manager/spawn.rs:25`): it
polls the viewport for up to 100ms (5ms interval) until a non-empty read, so a
caller that reads immediately after `spawn` sees content rather than a blank
screen. `read_viewport` drops trailing blank rows, so a non-empty result means
the child actually painted.

## Reading the screen

`render` always reads the active viewport, so scrollback is read by walking it
(`src/actor/engine.rs:186`): scroll the viewport to the top, render one row at a
time stepping down by one, then restore the bottom. Each row is joined into a
`String` with trailing blanks trimmed (`row_to_string`, `engine.rs:210`). An
all-blank viewport yields an empty `Vec` (`snapshot_to_viewport_lines`,
`engine.rs:220`), which is what `read_blocking` polls on for first paint.

`snapshot_to_styled_cells` (`engine.rs:271`) flattens the viewport into a
`rows x cols` `ndarray::Array2<StyledCell>`, mapping each `ix_vt::Cell`'s declared
style colors and SGR flags; an empty viewport is `NoOutputAvailable`, a shape
mismatch is `ArrayConversion`. `snapshot_to_cursor` (`engine.rs:299`) swaps the
snapshot's `(col, row)` into `CursorPos { row, col, visible }`.

## Tuning constants

`src/manager/spawn.rs`: `CHANNEL_BUFFER_SIZE = 100` (command mailbox depth),
`INITIAL_OUTPUT_TIMEOUT = 100ms`, `INITIAL_OUTPUT_POLL_INTERVAL = 5ms`. The PTY
read buffer is 8192 bytes (`src/actor/mod.rs:68`).
