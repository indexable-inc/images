# skill-lint

`packages/skill-lint` lints and autofixes `SKILL.md` files. It is a
non-panicking replacement for the `skillsaw` Python linter: it recursively finds
every `SKILL.md`, parses each file's frontmatter with a real YAML parser
(`serde_norway`), and reports diagnostics, never aborting on bad input
(`src/main.rs:1-6`). The motivating bug skillsaw had: a `description:` value with
a bare `: ` inside it is read by YAML as a nested mapping and breaks the document;
skillsaw reported that opaquely or panicked. skill-lint surfaces the precise YAML
parser error with a file line number.

```
skill-lint [PATH] [--format human|json]    # lint (default cmd)
skill-lint fix [PATH]                       # apply safe autofixes
```

`PATH` defaults to `.` (`src/main.rs:21-44`). A single `SKILL.md` file path is
accepted directly; a directory is walked gitignore-aware via the `ignore` crate's
`WalkBuilder` (respects `.gitignore`/global ignores, skips hidden dirs), so an
ignored or hidden `SKILL.md` is intentionally skipped (`find_skills`,
`src/main.rs:128-149`). `SKILL_FILE_NAME = "SKILL.md"` (`src/main.rs:19`).

## Modules

- `lint.rs`: pure analysis (frontmatter split, YAML parse, rules) producing
  `Diagnostic`s. No IO, no panics.
- `fix.rs`: pure safe autofixes producing new file contents.
- `main.rs`: CLI, filesystem walk, output, exit codes.

## Lint rules (`lint.rs`)

`lint_skill(path, contents)` (`src/lint.rs:129-243`) runs in order, short-
circuiting on structural failure:

| rule_id | severity | trigger |
| --- | --- | --- |
| `skill-frontmatter` | error | no leading `---` … `---` block; or invalid YAML (carries the parser message + file line); or frontmatter is not a mapping |
| `skill-name` | error | missing/empty `name` string |
| `skill-description` | error | missing/empty `description` string |
| `skill-name-matches-dir` | warning | `name` differs from the skill's directory name |
| `skill-description-length` | warning | description over `DESCRIPTION_MAX_CHARS = 1024` (`src/lint.rs:14`); injected verbatim into context, so overlong is wasted budget |
| `skill-file-budget` | warning | whole file over `FILE_TOKEN_BUDGET = 3000` estimated tokens, estimated as `len/4` (`src/lint.rs:19-22`) |

`Severity` is only `Error`/`Warning` (no `Info`; the workspace denies dead code,
so the unused variant is omitted until a rule needs it, `src/lint.rs:24-32`). A
`Diagnostic` is `{severity, path, line?, rule_id, message}` (`src/lint.rs:43-51`)
and renders as `severity path:line rule_id: message` (`render`, `:71-81`).

`split_frontmatter` (`src/lint.rs:100-125`) is shared with the fixer so both
agree on what counts as frontmatter. It tracks each line's byte span via
`split_inclusive` to slice the YAML exactly (no `\r\n` terminator-width drift)
and records `yaml_start_line = 2` so `serde_norway`'s 1-based parser line numbers
offset back to the right file line (`src/lint.rs:148-165`).

## Autofix (`fix.rs`)

`fix_skill(path, contents)` (`src/fix.rs:17-57`) is conservative and pure (no
IO). It refuses to touch a file whose frontmatter is missing or unparseable
(surfaced as a lint error instead, never a corrupted document,
`src/fix.rs:18-35`). On a valid mapping it:

- inserts `name: <dir>` as the first frontmatter field when `name` is absent
  (`insert_name`, `src/fix.rs:85-90`), so a re-lint comes back clean;
- normalizes whitespace: strips trailing whitespace per line, collapses trailing
  blank lines, ensures exactly one final newline (`normalize_whitespace`,
  `src/fix.rs:97-114`).

It preserves the file's dominant line ending: a CRLF-authored file keeps CRLF
(`str::lines()` drops `\r`, so it rejoins with the detected newline). `fix` is
idempotent: a second pass is a no-op (tests at `src/fix.rs:168-230`).

## Exit codes

`skill-lint` (lint) exits non-zero only when at least one error exists; warnings
and info never fail the gate (`run_lint`, `src/main.rs:97-103`). Operational
failures (unreadable path) are distinct: reported to stderr and exit non-zero
(`main`, `src/main.rs:59-67`). `fix` always exits 0 after applying changes
(`run_fix`, `src/main.rs:105-118`). `--format json` prints the diagnostics array
(`print_json`, `:120-124`).

## How it is built

`default.nix` selects the `skill-lint` binary via
`ix.cargoUnit.selectBinaryWithTests` (flake output `skill-lint`, `package.nix`):

```
nix run .#skill-lint -- skills
nix run .#skill-lint -- fix skills
```

It lints the skills under `skills/` (one directory per skill). Inline tests in
both `lint.rs` and `fix.rs` pin the rules and the conservative,
content-preserving fix behavior.
