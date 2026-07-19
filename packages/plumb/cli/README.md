<p align="center"><img src="assets/hero.svg" width="760" alt="a typed pipeline decomposes into tee'd stages; the run becomes an addressable value whose streams feed a later command without re-running"></p>

# plumb

Ever run `cargo clippy | tail -n 5` and then wanted the 400 lines you just threw away? plumb is a shell where every run is a value. Each pipe stage's stdout and stderr are captured while they stream, the whole run becomes an inspectable report, and every stream stays addressable afterwards, so later commands reuse earlier output (including the bytes that flowed *between* pipe stages) without re-running anything. It is a Rust library first (`plumb-core`), with a reedline REPL and one-shot CLI on top, built for programs (LLM agents especially) that run commands and need what actually happened, not a scrollback.

The syntax is a strict bash subset: everything plumb accepts pastes into bash unchanged and means the same thing there. Everything else is a loud parse error naming the construct, never a silent reinterpretation.

## Runs are values

```console
plumb src> echo warn >&2; seq 1 10000 | tail -n 3
warn
9998
9999
10000
[0] echo warn  exit 0  0ms (cpu 0+0ms)  out 0B  err 5B
[1] seq 1 10000  exit 0  9ms (cpu 4+2ms)  out 47.7KB  err 0B
[2] tail -n 3  exit 0  9ms (cpu 1+0ms)  out 16B  err 0B
run 3: exit 0  ${o[3]} ${e[3]} ${s[3]}
plumb src> echo ${o[3][1]} | head -n 1
1
```

Every run stays addressable:

| reference | value |
| --- | --- |
| `${o[7]}` / `${e[7]}` / `${s[7]}` | run 7's final stdout / stderr / status |
| `${o[7][0]}` | what pipe stage 0 printed (exactly what stage 1 consumed) |
| `${o[-1]}`, `${o[-1][-2]}` | negative indexes count back from the latest |
| `${runs[7].stages[0].stdout}` | structured paths, field names matching the report JSON (`output`, `status`, `argv`, `stdout_bytes`, `started_at_ms`/`ended_at_ms`, wall `duration_ms` and rusage `user_ms`/`sys_ms`, ...) |
| `$o` / `$e` / `$s` | the last run's stdout / stderr / status |
| `$?` | last status, as in bash |

`:runs` lists retained runs, `:json 7` dumps a run's full report (argv after expansion, per-stage status, timing, byte counts, truncation flags), `:out 7 0` prints a captured stream raw. Captures are head+tail bounded (256KiB per stream by default) with exact byte totals, so a gigabyte through a pipe costs fixed memory, and truncation is always marked, never silent.

## Strict where bash is treacherous

- Unset variable expansion is an error (no `rm -rf /$TYPO`).
- A glob matching nothing is an error (`failglob`).
- `pipefail` semantics, except SIGPIPE deaths (`yes | head` succeeds).
- Expansions never word-split: `$X` is exactly one argument.
- Unsupported bash (keywords, subshells, backticks, here-docs, fancy expansions) is a parse error with a span, so nothing runs under a wrong reading.
- State builtins (`cd`, `export`, ...) refuse to be pipeline stages instead of silently mutating a subshell.

Supported: pipes, `&&` `||` `;`, redirections (`>` `>>` `<` `2>` `2>&1` `&>` `>&2`), quoting, `$VAR`/`${VAR}`, `$(...)` command substitution, globs, `~`, `NAME=v cmd` prefixes, `&` background runs, comments.

## A library first

```rust
let shell = plumb_core::Shell::new(plumb_core::Config::default())?;
let report = shell.eval("cargo clippy 2>&1 | tail -n 5")?;
report.status;                        // pipefail status
report.pipelines[0].stages[0].stdout  // everything tail consumed
    .render();
shell.eval("echo ${o[1][0]} | rg dead")?;  // reused; clippy never re-runs
serde_json::to_string(&report)?;      // the whole run, machine-readable
```

`Shell` is a cheap cloneable handle over shared state: concurrent evals from threads (`eval_detached`) or `&` background items see each other's variables, cwd, and run history; `wait` joins them. `exit` surfaces as `Error::ExitRequested` so the embedder decides what dying means. From Elixir, `plumb-ex` (unibind) exposes the same shell with reports as JSON.

One-shot and script modes stream live and exit with the run's status; `--json` prints the report instead:

```console
$ plumb -c 'echo hi | tr a-z A-Z' --json | jq .pipelines[0].stages[1].stdout.text
"HI\n"
$ plumb build.plumb
$ some-generator | plumb
```

## Install

```sh
nix run github:indexable-inc/index#plumb
cargo install --git https://github.com/indexable-inc/plumb
```

The crates live in the [index monorepo](https://github.com/indexable-inc/index) under `packages/plumb/` (`plumb-syntax` parser, `plumb-core` library, `plumb` CLI, `plumb-ex` Elixir NIF); this repo is a read-only mirror.
