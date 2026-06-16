# agents-md

`packages/agents-md` is the Rust CLI that renders, diffs, checks, and writes the
always-on agent instruction files: `AGENTS.md` (Codex) and `CLAUDE.md` (Claude).
The crate keeps the name `agents-md`, but its flake output is `agent-context`
(`package.nix:5-9`) to match the `lib.agentContext` surface that owns both the
always-on instructions and the on-demand skills.

The actual instruction text is not in this crate. It is assembled by
`lib/agent-context` (nix-lib domain) from the fragments under the repo's
`agent-context/` tree and handed to this CLI through env at wrap time. The CLI is
a thin, content-agnostic renderer: it only knows "a list of (target, file_name,
generated_path) documents" and how to diff/check/write them.

## Purpose

`AGENTS.md`/`CLAUDE.md` are generated and gitignored, never committed
(`agent-context/README.md`). This CLI is the contributor convenience that writes
them to disk for preview (`nix run .#agent-context -- --write`) and the gate that
verifies an on-disk copy still matches the assembled source (`--check`).

## Public surface

CLI flags (`src/main.rs:16-41`, clap):

- `--target all|codex|claude` (default `all`): limit to one instruction target.
  `codex` maps to internal target string `"codex"`, `claude` to `"claude"`
  (`src/main.rs:50-58`).
- `--write [PATH]` (default missing value `.`): write generated files. With a
  directory, writes `<file_name>` into it; with a single selected target and a
  file path, writes that path.
- `--check [PATH]` (default missing value `.`): check on-disk files; exits
  non-zero if any differ or are symlinks.
- `--print`: print one generated file to stdout; requires a single target
  (`src/main.rs:211-220`).
- `--diff-renderer auto|plain|delta` (default `auto`): render the default
  no-mode diff as plain unified text or piped through `delta`.

The four flags `--write`, `--check`, `--print` are mutually exclusive; with none
set the default mode is `Diff` against the current directory (`mode_for`,
`src/main.rs:94-109`). Env inputs:

- `AGENTS_MD_DOCUMENTS` (required, `src/main.rs:13`): path to a JSON array of
  `{target, file_name, generated_path}` documents (`Document`,
  `src/main.rs:67-72`). Set by the wrapper.
- `AGENTS_MD_DELTA` (`src/main.rs:14`): path to the `delta` binary used by the
  TTY diff renderer; defaults to `delta` on PATH.

## Key behavior

- **Target inference from a path.** With `--target all` and a path whose file
  name matches a configured `file_name` (e.g. `AGENTS.md`), the CLI infers the
  single matching document (`infer_document_from_path`, `src/main.rs:255-260`); a
  directory path keeps all documents. A dotted directory that exists is treated
  as a directory, not a file (`existing_path_is_file`, `src/main.rs:280-282`).
- **Check is strict about symlinks.** `--check` treats a symlink at the target
  path as stale (`src/main.rs:173-178`); `--write` removes an existing symlink
  before writing a real file (`src/main.rs:348-350`). This matters because the
  skills package is copied rather than symlinked (Claude Code's autocomplete
  filters symlinks), and the same no-symlink policy is enforced here.
- **Diff output.** `diff_documents` builds a unified diff with headers
  `current/<path>` vs `generated/<file_name>` (`unified_diff`,
  `src/main.rs:297-304`); `auto` renderer pipes through `delta` only when stdout
  is a TTY, falling back to plain text if `delta` fails (`write_diff`,
  `src/main.rs:306-320`).
- **Empty/missing current file** reads as empty string for diffing, so the first
  write shows a full add rather than erroring (`read_current_text`,
  `src/main.rs:289-295`).

Tests in `src/main.rs:355-438` cover target inference, file-vs-directory path
resolution, and the dotted-directory edge cases.

## How it is built and wired

`default.nix` selects the `agents-md` binary via
`ix.cargoUnit.selectBinaryWithTests`, then wraps it with `ix.agentContext.mkApp`
(`lib/agent-context/default.nix:179-212`). `mkApp` writes each assembled document
to a `writeText` file, generates the `agent-context-documents.json` config, and
`makeWrapper`s the binary with `--set AGENTS_MD_DOCUMENTS <config>` and `--set
AGENTS_MD_DELTA <delta>`. The wrapped derivation's `mainProgram` stays
`agents-md`, so the command is `agents-md` even though the flake attr is
`agent-context`:

```
nix run .#agent-context -- --write          # write AGENTS.md + CLAUDE.md
nix run .#agent-context -- --check          # CI/contributor gate
nix run .#agent-context -- --target claude --print
```

## Relationship to agent-context

- `agent-context/` (repo root): the fragment tree. `sections/*.md` are fragments;
  `agents/` holds named sub-agent context. Each fragment's `disclosure:
  always|progressive` frontmatter decides whether it joins the always-on core
  (what this CLI writes) or becomes a skill (what [skill-lint](../skill-lint/overview.md)
  lints).
- `lib/agent-context` (nix-lib domain): parses the fragments, asserts the
  always-on character cap (`alwaysCharCap`), builds the `claude-md` / `codex-md`
  / `skills` outputs, and provides `mkApp`. This crate documents only the CLI;
  the assembly internals live in the nix-lib domain, not here.
- The `SessionStart` hook `agent-instructions.sh` (`.claude/hooks/`) is what
  actually delivers the core to a live session; this CLI is the disk-preview and
  check path, not the runtime injector.
