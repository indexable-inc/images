# git-log-pretty

`packages/git-log-pretty` is a pretty `git log` viewer. With no subcommand it
shows what HEAD is ahead of `main` (or recent history when HEAD is `main`), each
commit a one-line summary plus an icon tree of the files it touched; the `diff`
subcommand renders just the changed-file tree between two refs. On a TTY it pages
like `git log`, and on a graphics terminal it can draw each author's GitHub
avatar inline (`src/main.rs:1-10`).

- Crate: `git-log-pretty` (workspace member, MIT, `Cargo.toml:1-6`).
- Flake output: `nix run .#git-log-pretty`. Built by
  `ix.cargoUnit.selectBinaryWithTests` with `mainProgram = "git-log-pretty"`
  (`default.nix:3-9`); `package.nix` sets `flake`, `inRustWorkspace`,
  `passthruTests`.

## CLI surface (`src/main.rs:38-73`)

Global flags (apply to both the default log and `diff`):

| flag | default | effect |
| --- | --- | --- |
| `--no-pager` | off | write to stdout instead of paging (`src/main.rs:44-46`). |
| `--no-avatar` | off | never draw inline author avatars (`src/main.rs:47-49`). |
| `--avatar-rows <N>` | `2` | avatar height in terminal rows; `0` disables, capped at `64` (`src/main.rs:50-56`). |

Subcommand:

- `diff [BASE] [HEAD]` - changed-file tree between two refs, like `git diff
  --stat` with icons. `BASE` defaults to `main`, `HEAD` to `HEAD`
  (`src/main.rs:62-73`). Uses the merge-base (`base...head`) view, not a raw
  two-tree diff.

No subcommand runs the log view: on `main` it prints `recent_commits` capped at
`MAX_COMMITS = 15`; on any other branch it prints commits ahead of `main`, with
a `(showing first 15, N more hidden)` header when there are more, or
`All caught up with main` when there are none (`src/main.rs:92-145`,
`:29-32`).

## Modules (`src/main.rs:19-25`)

- **`git`** - repository queries on `git2`. `commits_ahead(repo, base)` is the
  set difference of commits reachable from HEAD minus those reachable from
  `base`, sorted newest-first (`src/git.rs:157-185`). `diff_stat_files` diffs the
  merge base against `head` so commits that landed on `base` after the fork do
  not pollute the tree (`src/git.rs:188-217`). `resolve_ref` tries
  `refs/heads/<name>`, `refs/remotes/<name>`, `refs/remotes/origin/<name>`, then
  the raw name, so a single-branch CI checkout still finds the base
  (`src/git.rs:106-125`). Each delta becomes a `ChangedFile { path, kind }` where
  `ChangeKind` collapses git's `Delta` into Added/Modified/Deleted/Renamed
  (`src/git.rs:13-33`).
- **`tree`** - renders changed files as a colored, icon-annotated tree. Paths
  fold into a directory trie, single-child directory chains collapse (so
  `a/b/c.rs` is one node), and each file gets its `devicons` glyph in the icon's
  own color; deletions render gray and struck through end to end
  (`src/tree.rs:1-6`, `:62-166`).
- **`display`** - the per-commit block: a `shorthash summary - relative-time`
  header plus the tree. A conventional-commit summary (`type(scope):
  description`) gets a hashed background "chip" on the type and a dimmed scope
  (`src/display.rs:21-66`). Avatar layout lays placeholder cells beside the text
  with `avatar_cols(rows) = 2 * rows` columns (`src/display.rs:137-213`).
- **`pager`** - routes output through `$PAGER` (run via `sh -c`, so `PAGER="less
  -R"` and empty-disables-paging both work), else `less`. For `less` it appends
  `-F -r -X` and defaults `$LESS` so Nerd Font glyphs pass through raw and stay in
  scrollback; a reader quitting early (`BrokenPipe`) is swallowed, not an error
  (`src/pager.rs:1-105`).
- **`palette`** - re-exports `terminal_theme::{Theme, detect}` and builds
  `anstyle` styles; light/dark picks foreground contrast and the devicons theme,
  and conventional-commit types get a stable hashed HSV background
  (`src/palette.rs:1-104`).
- **`avatar`** - GitHub login resolution and avatar fetch (below).
- **`time`** - coarse "N units ago" relative timestamps, "just now" under a
  minute and for future (clock-skewed) times (`src/time.rs:1-26`).

## Avatars: resolution and rendering

`avatar::Resolver` maps a commit author to PNG bytes cheapest-first and ties
each step to a real account (`src/avatar.rs:1-9`, `:88-129`):

1. an explicit `git config githubLogin.map` override (`email=login` multivar,
   `src/avatar.rs:260-283`),
2. the login embedded in a GitHub `noreply` email, then
3. GitHub's record of who authored the commit (origin remote slug + commit SHA).

Caches are aggressive so a log view makes at most one network request per unique
author: an in-memory `login`/`png` cache, an on-disk PNG cache, and a persisted
`email\tlogin` map (`logins.tsv`) under
`$XDG_CACHE_HOME`/`$HOME/.cache` `git-log-pretty/avatars`
(`src/avatar.rs:285-311`). A GitHub token (for the authenticated lookups, not the
download) is read from `GITHUB_TOKEN`, `GH_TOKEN`, then `gh auth token`
(`src/avatar.rs:240-258`). The async client runs on a short-lived current-thread
tokio runtime (`src/main.rs:239-263`).

Rendering uses the kitty Unicode-placeholder graphics protocol. Each unique
avatar's pixels are transmitted once up front via `kitty::transmit_virtual`, then
the commit gutter is drawn with placeholder cells that resolve to that image id
(image ids are 24-bit, seeded per-pid, masked by `ID_MASK`,
`src/avatar.rs:20-22`, `:155-174`). Because placeholder cells are ordinary
left-to-right text, the log scrolls like any output, but they must reach the
terminal verbatim, so a graphics log bypasses the pager (a screen-repainting
pager severs the cell from the foreground color carrying its image id);
a plain log still pages (`src/main.rs:147-215`). Avatars draw only when
`--avatar` is on, `avatar_rows > 0`, `kitty::is_supported()`, and stdout is a
terminal; any fetch or transmit failure falls back to the plain renderer
(`src/main.rs:171-188`).

## Dependencies and build

`anstyle`, `chrono`, `clap` (derive), `color-eyre`, `devicons` (external `0.6`),
sibling workspace crates `github-avatar`, `kitty`, `terminal-theme`,
`strip-ansi-escapes`, and `tokio` (`rt`/`net`/`time`). `git2` is built with
`default-features = false` + `vendored-libgit2`, dropping the https/ssh
transports and compiling bundled libgit2 with `cc`, so no system libgit2 or cmake
is needed (`Cargo.toml:11-29`).

`tests/integration.rs` drives the built binary against temporary `git2` repos
(ahead-of-main, on-main, detached, `diff` ranges); it is the passthru test target
(`tests/integration.rs:1-7`, `default.nix`).
