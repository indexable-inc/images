<p align="center"><img src="assets/hero.svg" width="720" alt="a pipeline whose every stream is tapped into a run value, whose variables feed a later command"></p>

# plumb

Ever run `cargo clippy | tail -n 5` and then wanted the 400 lines you just threw away? plumb is a shell where every run is a value: each pipe stage's stdout and stderr are captured (bounded, with exact byte counts), the whole run becomes an inspectable report, and outputs auto-bind to variables so later commands can reuse any earlier stream, including the intermediate pipe data, without re-running anything. It is a Rust library first (`plumb-core`), with a reedline REPL and a one-shot CLI on top, built for programs (LLM agents especially) that run commands and need what actually happened, not a scrollback.

The syntax is a strict bash subset: everything plumb accepts pastes into bash unchanged and means the same thing there. Everything else is a loud parse error naming the construct, never a silent reinterpretation.

## Runs are values

```console
plumb tmp> sh -c 'echo warn >&2; seq 1 10000' | tail -n 3
9998
9999
10000
[0] sh -c echo warn >&2; seq 1 10000  exit 0  11ms  out 48.9KB  err 5B
[1] tail -n 3  exit 0  11ms  out 15B  err 0B
run 3: exit 0  ${o3} ${e3} ${s3}
plumb tmp> echo $o3_0 | head -n 1
1
```

Every run `N` binds, automatically:

| variable | value |
| --- | --- |
| `$oN` / `$eN` / `$sN` | final stdout / stderr / status |
| `$oN_K` / `$eN_K` | stdout / stderr of pipe stage `K` (what stage `K+1` consumed) |
| `$o` / `$e` / `$s` | same, for the most recent run |
| `$?` | last status, as in bash |

`:runs` lists retained runs, `:json N` dumps a run's full report (argv after expansion, per-stage status, timing, byte counts, truncation flags), `:out N K` prints a captured stream raw. Captures are head+tail bounded (256KiB per stream by default) with exact totals, so a gigabyte through a pipe costs fixed memory and truncation is always marked, never silent.

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
report.status;                       // pipefail status
report.pipelines[0].stages[0].stdout // everything tail consumed
    .render();
shell.var("o1_0");                   // same thing, as a variable
serde_json::to_string(&report)?;     // the whole run, machine-readable
```

`Shell` is a cheap cloneable handle over shared state: concurrent evals from threads (`eval_detached`) or `&` background items see each other's variables, cwd, and run history; `wait` joins them. `exit` surfaces as `Error::ExitRequested` so the embedder decides what dying means.

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

The crates live in the [index monorepo](https://github.com/indexable-inc/index) under `packages/plumb/` (`plumb-syntax` parser, `plumb-core` library, `plumb` CLI); this repo is a read-only mirror.
