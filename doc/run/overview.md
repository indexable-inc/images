# run

`packages/run` executes a command under a recorded PTY session and keeps the
output: a full log, a replayable cast, and queryable structured events, while
keeping printed output small enough for agent logs by default. It is a single
Python script (`run.py`, 912 lines) wrapped by Nix as the flake output
`nix run .#run` (`package.nix:3-4`), not part of the Cargo workspace.

```sh
nix run .#run -- nix build .#base   # everything after `run` is the command, verbatim
```

The first argument is the command; every following argument is passed through
unchanged (`run.py:900`, `usage` at `:640`).

## What it does

`run` opens its own PTY (`pty.openpty`, `run.py:476`), forks the command onto the
slave as a new session leader (`setsid` + `TIOCSCTTY`, `:481-489`), and copies
the master's output to disk and (partially) to the caller's stdout. It forwards
the caller's stdin to the child on a daemon thread (`forward_stdin`, `:584`), puts
the caller's tty in raw mode while attached (`:786`), forwards `SIGINT`/`SIGTERM`/
`SIGHUP` to the child's process group, and on `SIGWINCH` resizes the PTY and
re-signals the child (`resize`/`forward`, `:805-821`). The main loop reads the
PTY non-blocking via a selector, reaps the child, and finalizes the summary
(`run`, `:718`).

## Recorded files (one session dir per run)

Sessions land under `./.ix/run/<timestamp>-<slug>-<pid>/` with `./.ix/run/latest`
symlinked to the newest (`session_paths`, `run.py:347`). Each session writes
(`ArtifactPaths`, `:38`):

- `output.log`: full terminal output, flushed live.
- `typescript` + `timing.log`: the `scriptreplay` pair.
- `session.cast`: an asciinema v2 stream (header carries terminal size, command,
  env; one `["o", text]` event per chunk, `Recorder._write_cast_header`, `:281`).
- `events.jsonl`: one JSON object per PTY output chunk (elapsed time, byte count,
  decoded text, base64 bytes, `Recorder.record`, `:241`).
- `lines.jsonl`: one object per completed output line, shaped for
  `polars.read_ndjson` (`LineRecorder`, `:145`).
- `summary.json`: command, cwd, terminal metadata, artifact paths, limits,
  duration, and exit status; written first as `running`, then replaced with the
  final result (`initial_summary` `:675`, finalized `:874`). Exit status maps a
  signal to `128 + signum` (`status_code`, `:615`).
- `replay` + `live`: generated helper scripts (`replay [divisor]` runs
  scriptreplay; `live` is `tail -f output.log`, `write_helper_scripts`, `:405`).

## Printed-output limiting

By default `run` prints the first 80 and last 80 output lines and records the rest
to `output.log`, so a long build does not flood an agent log (`DisplayLimiter`,
`run.py:73`; defaults `DEFAULT_HEAD_LINES`/`DEFAULT_TAIL_LINES = 80`, `:29`). When
output exceeds the head limit it announces the live-stream path on stderr and
buffers the tail; on finish it prints the tail with a count of omitted middle
lines.

## Environment knobs

- `IX_RUN_HEAD_LINES` / `IX_RUN_TAIL_LINES`: lines to print (`env_int`, `:313`).
- `IX_RUN_PRINT`: `summary` (default), `full` (mirror every line), or `none`
  (`print_mode`, `:326`).
- `IX_RUN_DIR`: session root, default `./.ix/run` (`state_root`, `:334`).
- `IX_RUN_SCRIPTREPLAY`: the scriptreplay binary, set by the Nix wrapper
  (`replay_command`, `:392`).

## Build

`default.nix` wraps `run.py` with `ix.writePythonApplication` and uses
`makeWrapper` to bake `IX_RUN_SCRIPTREPLAY` to `util-linux`'s `scriptreplay` on
Linux (`default.nix:8-32`), so replay works without the binary on `PATH`. The
`recordsSession` passthru test drives the wrapped binary through a real run and
asserts every artifact exists, that summarized stdout shows head+tail but not the
middle, and that stdin forwarding (redirected, non-blocking, and closed) and a
closed stdout all behave (`default.nix:33-184`). `meta.mainProgram = "run"`.
