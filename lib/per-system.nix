# Per-system flake outputs (packages / checks / formatter).
#
# Kept out of flake.nix so the flake top-level can read as a manifest of
# inputs and output categories. Composition logic for workflow tools and
# lint plumbing lives here. Workflow tools (lint, update-mods, ...) are
# exposed under `packages.<system>.<name>` with `meta.mainProgram` set, so
# `nix run .#<name>` and `nix build .#<name>` both work without an `apps`
# entry (see AGENTS.md "Flake.nix style").
{
  system,
  ix,
  nixpkgs,
  paths,
  rust-overlay,
  home-manager,
}: let
  inherit (nixpkgs) lib;
  pkgs = import nixpkgs {
    inherit system;
    config = {};
    overlays = [
      rust-overlay.overlays.default
      ix.overlay
    ];
  };
  fs = lib.fileset;
  packageRegistry = import (paths.packagesRoot + "/registry.nix") {
    inherit lib;
    root = paths.packagesRoot;
    inherit (ix.lists) findDuplicates;
  };

  # Each lint stage is one subcommand on a single binary so the spec keys
  # off `lib.getExe lintStage` without registering four sibling packages.
  # The Nu wrapper checks syntax at build time, so a typo in a stage shows
  # up in the `lint` derivation build, not at `nix run` time.
  lintStage = ix.writeNushellApplication pkgs {
    name = "lint-stage";
    meta.description = "One lint stage (alejandra | statix | deadnix | astlog | astlog-rust | astlog-elixir | shell-fence | filenames | dirnames | svg-dark | site-ids | ruff | clone); driven by `lint`";
    runtimeInputs = [
      pkgs.alejandra
      pkgs.deadnix
      pkgs.fd
      pkgs.ruff
      pkgs.statix
      repoPackages.astlog
      repoPackages.clone
    ];
    text = ''
      # nu
      def "main alejandra" [] {
        let nix_files = (fd --extension nix | lines)
        alejandra --check ...$nix_files
      }
      def "main statix" [] { statix check . }
      # Strict: no `-L`/`--no-lambda-pattern-names`. That flag exists because
      # dropping a pattern name is unsafe without `...` in the pattern (it
      # narrows the callable signature); an unused name here must be deleted
      # (migrating call sites) or kept behind `...`, matching what the LSP
      # already flags as unused.
      def "main deadnix" [] { deadnix --fail . }
      # The Nix style rules as astlog lint declarations
      # (astlog-rules/nix.astlog, #1060/#1062). `astlog scan` emits one
      # finding per lint-declared relation row and exits nonzero on any
      # error-severity finding, so adding a (lint ...) extends the gate
      # without touching this invocation. Legitimate exceptions are
      # suppressed in place with `astlog-ignore: <rule>` comments. Only
      # .nix files are handed to the corpus: astlog would otherwise parse
      # every known-grammar file in the repo to run nix-only rules.
      def "main astlog" [] {
        let nix_files = (fd --extension nix | lines)
        astlog scan astlog-rules/nix.astlog ...$nix_files
      }
      # The Rust style rules (astlog-rules/rust.astlog), the successor to the
      # ast-grep rust rules (#1060 ported the nix rules first). Scoped to the
      # corpus/search crates, the `files:` scope those rules carried under
      # ast-grep; astlog walks each directory and runs the rust rules over its
      # .rs files. Both rulesets share the `astlog-rules` flake-check self-test.
      def "main astlog-rust" [] {
        let dirs = (
          [
            packages/indexer
            packages/search
            packages/search-core
            packages/search-py
            packages/source
            packages/sink
          ]
          | where {|d| $d | path exists}
        )
        if ($dirs | is-not-empty) {
          astlog scan astlog-rules/rust.astlog ...$dirs
        }
        # The Cargo/workspace rules (astlog-rules/cargo.astlog, TOML grammar) run
        # over every Cargo.toml in the repo: `no-cargo-path-dep` bans inter-crate
        # `path` deps in member tables so local crates are declared once in a
        # [workspace.dependencies] and inherited with `workspace = true`. A
        # separate ruleset because the `astlog-rules` self-test maps one source
        # extension per ruleset (rust.astlog -> .rs, cargo.astlog -> .toml).
        let cargo_files = (fd --hidden --glob Cargo.toml | lines)
        if ($cargo_files | is-not-empty) {
          astlog scan astlog-rules/cargo.astlog ...$cargo_files
        }
      }
      # The Elixir lint rules (astlog-rules/elixir.astlog), two families. Type
      # discipline: a struct needs a `@type`, a public `def` needs a preceding
      # `@spec` (behaviour callbacks marked `@impl` are exempt), and a module
      # needs a `@moduledoc` — the lint-level nudge toward the shape Elixir
      # 1.18's set-theoretic checker can check. Correctness/security: no unsafe
      # dynamic atom creation (atom-table DoS), no leftover `IO.inspect`. Run
      # over every package's `lib/` Elixir, not a hand-maintained directory list:
      # the only scoping is to `lib/` itself, because `mix.exs` build functions
      # and `test/` ExUnit helpers are not the type-checked runtime surface and
      # speccing them would be noise. `fd` already skips gitignored `_build`/`deps`.
      def "main astlog-elixir" [] {
        let files = (
          fd --extension ex --extension exs
          | lines
          | where {|p| $p =~ '(^|/)lib/' }
        )
        if ($files | is-not-empty) {
          astlog scan astlog-rules/elixir.astlog ...$files
        }
      }
      # The shell fence (#3823, phase 1): no NEW generated shell or nushell.
      # Call sites of the write*Application / write*Script builders (matched
      # as AST identifiers by astlog-rules/shell-fence.astlog, so comments
      # never count) and committed .sh/.bash/.nu files are frozen behind
      # shell-allowlist.txt: an entry is `path` for a script file or
      # `path:identifier:count` for the call sites in one .nix file. A
      # scanned occurrence with no matching entry fails (write a compiled
      # Rust tool instead), and an entry whose target shrank or vanished
      # fails too, so the allowlist only shrinks as scripts migrate to Rust.
      # Counts rather than line numbers so unrelated edits that shift lines
      # do not churn the allowlist, while a new call site in an already
      # listed file still trips the fence.
      def "main shell-fence" [] {
        let allowed = (
          open --raw shell-allowlist.txt
          | lines
          | each {|line| $line | str trim }
          | where {|line| $line != "" and not ($line | str starts-with "#") }
        )
        let scripts = (
          fd --hidden --type file --exclude .git --exclude .claude/worktrees
            --extension sh --extension bash --extension nu
          | lines
        )
        let nix_files = (fd --extension nix | lines)
        # `astlog scan` exits nonzero on findings by design; capture the JSON
        # (an empty corpus still prints `[]`) instead of failing the stage.
        let scan = (do { astlog scan astlog-rules/shell-fence.astlog --json ...$nix_files } | complete)
        if ($scan.stdout | str trim | is-empty) {
          print --stderr $scan.stderr
          error make { msg: "astlog scan emitted no JSON" }
        }
        let call_sites = (
          $scan.stdout
          | from json
          | group-by {|f| $"($f.file):($f.text)" }
          | transpose site findings
          | each {|row| $"($row.site):($row.findings | length)" }
        )
        let actual = ($scripts | append $call_sites | sort)
        let new = ($actual | where {|entry| $entry not-in $allowed })
        let stale = ($allowed | where {|entry| $entry not-in $actual })
        if ($new | is-not-empty) {
          print --stderr "new shell/nushell is fenced (#3823); write a compiled Rust tool instead (ix.rustWorkspace; see packages/config-launch, packages/claude-hooks). Not in shell-allowlist.txt:"
          $new | each {|entry| print --stderr $"  ($entry)" }
        }
        if ($stale | is-not-empty) {
          print --stderr "stale shell-allowlist.txt entries (script gone or call-site count changed); the allowlist only shrinks, update it:"
          $stale | each {|entry| print --stderr $"  ($entry)" }
        }
        if (($new | is-not-empty) or ($stale | is-not-empty)) { exit 1 }
      }
      # Repository configuration belongs in composable Nix expressions. Keep
      # serialized files only where an external consumer owns the filename or
      # the file is generated data, a lock, a fixture, or a protocol payload.
      def "main filenames" [] {
        let allowed = [
          # Ecosystem-owned configuration and manifests.
          '(^|/)Cargo\.toml$'
          '(^|/)pyproject\.toml$'
          '(^|/)rust-toolchain\.toml$'
          '(^|/)mise\.toml$'
          '(^|/)osv-scanner\.toml$'
          '(^|/)ruff\.toml$'
          '(^|/)statix\.toml$'
          '(^|/)\.cargo/config\.toml$'
          '^clone\.toml$'
          '^packages/cve-scan/whitelist\.toml$'
          '^\.github/.*\.ya?ml$'
          '(^|/)docker-compose\.ya?ml$'
          '(^|/)plugin\.yml$'
          '^\.editorconfig$'
          '^packages/minecraft/minestom/servers/[^/]+/gradle\.properties$'
          '^packages/minecraft/minestom/servers/[^/]+/gradle/verification-metadata\.xml$'
          '^packages/minecraft/minestom/servers/[^/]+/src/main/resources/logback\.xml$'
          # Gradle owns these root-build names; the catalog and verification
          # metadata are generated inputs shared by the Minestom subprojects.
          '^packages/minecraft/minestom/gradle\.properties$'
          '^packages/minecraft/minestom/gradle/libs\.versions\.toml$'
          '^packages/minecraft/minestom/gradle/verification-metadata\.xml$'
          '^packages/minecraft/minestom/gradle/snapshot-metadata\.xml$'

          # Generated manifests, locks, editor settings, and typed data.
          '(^|/)(package|tsconfig)\.json$'
          # TypeScript's project-variant convention: tsconfig.VARIANT.json.
          # Vite scaffolds ship tsconfig.node.json, mkapp adds
          # tsconfig.staging.json; tsc owns these names via `-p`.
          '(^|/)tsconfig\.[a-z-]+\.json$'
          '(^|/)(package-lock|lock)\.json$'
          '(^|/)(pins|manifest)\.json$'
          '^\.(claude|vscode|zed)/settings\.json$'
          '^\.vscode/extensions\.json$'
          '^\.github/user-owners\.json$'
          '(^|/)(dag|upstream-status)\.json$'
          '(^|/)(fixtures?[^/]*|snapshots?|catalogs?|metadata|sounds|seeds)/.*\.json$'
          '^examples/.*\.json$'
          '^packages/agent/claude-code/system-prompts/models\.json$'
          # Byte-patch mapping data (find/replace pairs) consumed as JSON
          # store paths by claude-code-rainbow's patch-binary.py.
          '^packages/agent/claude-code-rainbow/mappings/[^/]+\.json$'
          '^packages/agent/system-prompt-eval-viewer/src/sample\.json$'
          # URL + hash pin data for the system-card PDF corpus (repo pin
          # policy keeps fetch hashes in data, not inline nix).
          '^packages/system-cards/catalog\.json$'
          '^packages/code/code-highlight/src/islands-theme\.json$'
          # Generated by `tree-sitter generate` and embedded by the grammar
          # crate's lib.rs (see packages/code/tree-sitter-nix/README.md).
          '^packages/code/tree-sitter-nix/src/node-types\.json$'
          # Live upstreaming state written by upstream-sync (PR urls, states,
          # retirement); generated, never hand-written. See that package.
          '^packages/upstream-sync/status/.*\.json$'
          '^tests/.*\.json$'
        ]
        let candidates = (
          fd --hidden --type file
          --extension toml --extension json --extension yaml --extension yml
          --extension kdl --extension ini --extension conf --extension cfg --extension xml
          --extension properties --extension editorconfig --extension sobelow-conf
          --exclude .git --exclude .claude/worktrees
          | lines
        )
        let denied = ($candidates | where {|path| not ($allowed | any {|pattern| $path =~ $pattern})})
        if ($denied | is-not-empty) {
          print --stderr "prefer .nix for repository-owned configuration; serialized files require an external filename or generated/data role:"
          $denied | each {|path| print --stderr $"  ($path)" }
          exit 1
        }
      }
      # A grouping directory must never restate its parent's name — the
      # directory-tree form of the scopedNaming rule. The one occurrence,
      # packages/minecraft/minecraft/{bot,nbt,...}, was flattened into
      # packages/minecraft (b32885d); this stage keeps the doubled segment
      # from coming back. Scoped to consecutive duplicates in the grouping
      # hierarchy only: a package root (a `package.nix` or `default.nix`
      # marker, the same markers packages/registry.nix discovers by) and
      # everything beneath it is exempt, because an eponym package inside
      # its area (packages/nix/nix) is deliberate and language layouts
      # inside a package (the mcp server's Python src/slack/slack) repeat a
      # segment by convention. Non-consecutive repeats (foo/bar/foo) are
      # fine and stay out of scope.
      def "main dirnames" [] {
        let offenders = (
          fd --type directory . packages
          | lines
          | where {|dir| ($dir | path basename) == ($dir | path dirname | path basename) }
          | where {|dir|
              let segments = ($dir | path split)
              let enclosing = (1..($segments | length) | each {|n| $segments | first $n | path join })
              not ($enclosing | any {|scope|
                ["package.nix" "default.nix"] | any {|marker| ($scope | path join $marker | path exists) }
              })
            }
        )
        if ($offenders | is-not-empty) {
          print --stderr "grouping directory restates its parent's name; flatten the child into its parent:"
          $offenders | each {|dir| print --stderr $"  ($dir)" }
          exit 1
        }
      }
      # Safari never evaluates `prefers-color-scheme` (or `light-dark()`)
      # inside an SVG loaded via `<img>` (WebKit bug 199134), so a
      # self-adapting SVG embedded bare renders its light palette for Safari
      # readers on GitHub's dark theme. Any markdown embed of such an SVG must
      # go through a `<picture>` whose dark source points at a committed
      # `-dark.svg` twin (the creating-a-readme skill documents the pattern).
      def "main svg-dark" [] {
        let offenders = (
          fd --hidden --extension md --exclude .git --exclude .claude
          | lines
          | each {|md|
              let dir = ($md | path dirname)
              let body = (open --raw $md)
              $body
              | parse --regex '(?:src="|\]\()(?<ref>[^"()\s]+\.svg)'
              | each {|row| $row.ref }
              | uniq
              | each {|ref|
                  let svg = (if ($ref | str starts-with '/') { $ref | str substring 1.. } else { $dir | path join $ref })
                  let dark_ref = ($ref | str replace --regex '\.svg$' '-dark.svg')
                  let dark_svg = ($svg | str replace --regex '\.svg$' '-dark.svg')
                  let adaptive = (($svg | path exists) and ((open --raw $svg) =~ 'prefers-color-scheme|light-dark\('))
                  let covered = (($body | str contains $'srcset="($dark_ref)"') and ($dark_svg | path exists))
                  if ($adaptive and (not $covered)) { $"  ($md) embeds ($ref)" } else { null }
                }
              | compact
            }
          | flatten
        )
        if ($offenders | is-not-empty) {
          print --stderr "Safari ignores prefers-color-scheme inside img-loaded SVGs (WebKit bug 199134); embed via <picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"<hero>-dark.svg\"><img src=\"<hero>.svg\"></picture> with a committed dark twin:"
          $offenders | each {|line| print --stderr $line }
          exit 1
        }
      }
      # Plan/update/story identity (packages/site/src/lib/plans, updates,
      # stories) is enforced at SvelteKit build time -- plans.ts throws on a
      # duplicate plan number or a frontmatter/filename mismatch -- but the
      # Check gate never builds the site, so two individually-green PRs merged
      # into a duplicate plan number 0031 and main's site build stayed red for
      # 8.5h before the Pages workflow surfaced it (#3669). This stage
      # front-loads the pure file-scan invariants into the gate every PR and
      # main commit runs: within each content directory a four-digit filename
      # prefix is claimed at most once, frontmatter `id` equals the filename
      # stem (stems are unique per directory, so ids stay unique), and a
      # frontmatter `number` agrees with the prefix (the #3668 rename fixed
      # the collision but left the old `number` behind, which kept the site
      # build red). The site build remains the full validator.
      def frontmatter-field [front: list<string>, key: string] {
        let matches = ($front | parse --regex ('^' + $key + ': *(?<value>.+)$'))
        if ($matches | is-empty) { null } else {
          $matches | get 0.value | str trim | str trim --char "'"
        }
      }
      def "main site-ids" [] {
        let errors = (
          [plans updates stories]
          | each {|kind|
              let entries = (
                fd --extension svx . $"packages/site/src/lib/($kind)"
                | lines
                | sort
                | each {|path|
                    let stem = ($path | path parse | get stem)
                    let front = (open --raw $path | lines | skip 1 | take until {|line| $line == '---'})
                    {
                      path: $path
                      stem: $stem
                      id: (frontmatter-field $front id)
                      number: (frontmatter-field $front number)
                      prefix: ($stem | parse --regex '^(?<n>\d{4})-' | get -o 0.n)
                    }
                  }
              )
              let id_mismatches = (
                $entries
                | where {|e| $e.id != $e.stem}
                | each {|e| $"  ($e.path): frontmatter id '($e.id)' disagrees with filename '($e.stem)'"}
              )
              let number_mismatches = (
                $entries
                | where {|e| $e.number != null and $e.number != $e.prefix}
                | each {|e| $"  ($e.path): frontmatter number '($e.number)' disagrees with filename prefix '($e.prefix)'"}
              )
              let numbered = ($entries | where {|e| $e.prefix != null})
              let duplicate_prefixes = (
                if ($numbered | is-empty) { [] } else {
                  $numbered
                  | group-by prefix
                  | transpose prefix claimants
                  | where {|row| ($row.claimants | length) > 1}
                  | each {|row| $"  number ($row.prefix) claimed by (($row.claimants | get path | str join ' and '))"}
                }
              )
              let identified = ($entries | where {|e| $e.id != null})
              let duplicate_ids = (
                if ($identified | is-empty) { [] } else {
                  $identified
                  | group-by id
                  | transpose id claimants
                  | where {|row| ($row.claimants | length) > 1}
                  | each {|row| $"  id ($row.id) claimed by (($row.claimants | get path | str join ' and '))"}
                }
              )
              [$id_mismatches $number_mismatches $duplicate_prefixes $duplicate_ids] | flatten
            }
          | flatten
        )
        if ($errors | is-not-empty) {
          print --stderr "site content identity is enforced by the SvelteKit build, which the Check gate never runs (#3669); keep filenames and frontmatter agreeing so collisions fail fast:"
          $errors | each {|line| print --stderr $line }
          exit 1
        }
      }
      # Repo-wide Python lint: the shared ruff selector (bug-catchers + security +
      # pathlib + pytest + explicit annotations + no `typing.cast`; see
      # lib/ruff-ann.nix) over EVERY tracked .py, so non-package dirs
      # (tools/, users/, skills/, sdk/, examples/, lib/) are covered too, not just
      # the per-package build gates. `fd` skips gitignored paths; `.claude` (agent
      # worktrees and assets) is filtered out explicitly.
      def "main ruff" [] {
        let py_files = (
          fd --extension py
          | lines
          | where {|p| not ($p | str starts-with ".claude/") }
        )
        if ($py_files | is-not-empty) {
          ruff check ${ix.ruffAnnArgs} ...$py_files
        }
      }
      # Code clone detection over the whole tree (packages/code/clone-detect).
      # `clone .` walks up for the repo `clone.toml`, whose `[budget]
      # global_pct` is the ceiling on whole-scan `duplication_pct`; the binary
      # exits nonzero when the global gate fails, so this gate ratchets
      # duplication down without failing on every pre-existing clone. Only the
      # global gate runs here: the diff gate needs a `.git` directory, and the
      # CI lint derivation copies a `.git`-less source tree. `clone` prints the
      # DetectionResult JSON to stdout; redirect it to null so a failing stage's
      # log shows the tracing gate summary (stderr), not the full JSON blob.
      def "main clone" [] {
        clone . out> /dev/null
      }
      def main [] {
        error make { msg: "specify a stage: alejandra | statix | deadnix | astlog | astlog-rust | astlog-elixir | shell-fence | filenames | dirnames | svg-dark | site-ids | ruff | clone" }
      }
    '';
  };

  # One stage list drives both the dag spec (default human path) and the
  # `--json` runner inside `lint`, so adding a stage cannot update one path
  # and silently miss the other.
  lintStages = [
    "alejandra"
    "statix"
    "deadnix"
    "astlog"
    "astlog-rust"
    "astlog-elixir"
    "shell-fence"
    "filenames"
    "dirnames"
    "svg-dark"
    "site-ids"
    "ruff"
    "clone"
  ];

  lintSpec = (pkgs.formats.json {}).generate "lint-dag.json" {
    nodes = lib.genAttrs lintStages (stage: {
      command = [
        (lib.getExe lintStage)
        stage
      ];
    });
  };

  lint = ix.writeNushellApplication pkgs {
    name = "lint";
    meta.description = "Run all Nix formatting and lint checks in parallel via dag-runner; `--fix` applies the fixer lanes to the worktree";
    runtimeInputs = [
      pkgs.git
      repoPackages.dag-runner
    ];
    text = ''
      # nu
      const stages = ${builtins.toJSON lintStages}
      const stage_bin = "${lib.getExe lintStage}"

      def --wrapped main [...args] {
        # `--json` (#1683) emits one JSON document — [{check, ok, output}] —
        # so agents can load lint results as a dataframe instead of grepping
        # the human log. It runs the same stage binary the dag spec points
        # at; dag-runner is bypassed only because its json mode is an NDJSON
        # event stream that drops the captured diagnostics. Exit code matches
        # the dag-runner contract: the worst stage exit code.
        if "--json" in $args {
          if ($args | length) > 1 {
            error make { msg: "--json takes no other arguments" }
          }
          let runs = (
            $stages
            | par-each --keep-order {|stage|
                let r = (do { ^$stage_bin $stage } | complete)
                {
                  check: $stage
                  ok: ($r.exit_code == 0)
                  # `ansi strip` because the stages color their diagnostics and
                  # nushell's `to json` passes raw ESC bytes through unescaped,
                  # which strict parsers (jq) reject as invalid JSON.
                  output: (($r.stdout + $r.stderr) | ansi strip)
                  exit_code: $r.exit_code
                }
              }
          )
          print ($runs | reject exit_code | to json)
          exit ($runs | get exit_code | math max)
        }
        # `--fix` (#3432) applies the fixer lanes to the worktree: build
        # `.#lint-fix-patch` -- the unified diff from the lanes' input
        # snapshot to the composed fixed tree (see `lintFix` below) -- and
        # `git apply` it from the repo root. A patch rather than a copy of
        # the fixed files is a safety property: the snapshot Nix evaluates
        # can be older than the worktree (a linked git worktree evaluates
        # the committed state), and `git apply` is all-or-nothing on context
        # mismatch, so stale fixes refuse loudly instead of clobbering
        # uncommitted edits. `nix` comes from the ambient PATH for the same
        # reason as `.#check`: a pinned client could mismatch the host daemon.
        if "--fix" in $args {
          if ($args | length) > 1 {
            error make { msg: "--fix takes no other arguments" }
          }
          # Both the flake ref and `git apply` paths are root-relative, so
          # anchor at the repo root instead of requiring callers to be there.
          cd (^git rev-parse --show-toplevel | str trim)
          let patch = (
            ^nix ...[
              "build" ".#lint-fix-patch"
              "--no-link" "--print-out-paths"
              "--option" "extra-experimental-features" "ca-derivations"
            ]
            | str trim
          )
          if (open --raw $patch | is-empty) {
            print "lint --fix: tree already clean, nothing to apply"
          } else {
            ^git apply --stat $patch
            ^git apply $patch
            print "lint --fix: applied; the verdict-only stages (astlog, shell-fence, filenames, dirnames, clone) and unfixable findings still need `nix run .#lint`"
          }
          exit 0
        }
        exec dag-runner ...$args ${lintSpec}
      }
    '';
  };

  # Fixer lanes (#3431/#3432): the fixable lint stages recast as pure tree
  # transformers `src -> src'`. A derivation cannot mutate the repo, so
  # "apply the fixes" means: emit the fixed tree, and let each consumer diff
  # it against what it has (`lint --fix` above today, the CI autofix commit
  # of #3435 later). Lanes split by file domain and build concurrently as
  # independent derivations (wall clock = slowest lane); within a lane the
  # stages run sequentially with the formatter LAST, because fix stages emit
  # unformatted edits -- same-lane fixes computed in parallel against the
  # original tree could union cleanly and still fail the format check.
  # Every derivation here is content-addressed, so a no-op fix realises to
  # its input's content and an already-clean tree is cache hits all the way
  # down to an empty patch.
  lintFix = let
    contentAddressed = {
      __contentAddressed = true;
      outputHashAlgo = "sha256";
      outputHashMode = "recursive";
    };
    # The repo pin (rust-toolchain.toml, via lib/rust/tooling.nix) plus the
    # rustfmt component its component list omits. Bound once and exported so
    # the acceptance check below runs `cargo fmt --check` with byte-for-byte
    # the same cargo-fmt the lane fixes with.
    rustFmtToolchain = ix.repoRustToolchainFor pkgs {
      components = [
        "cargo"
        "rustc"
        "rustfmt"
      ];
    };
    # One lane: copy the scoped source, run the lane's fixers in place, emit
    # the tree. The fixers see nothing outside their scoped `src` by
    # construction, so lane outputs stay as disjoint as lane inputs.
    mkLane = {
      name,
      tools,
      fix,
    }: src:
      pkgs.runCommand "lint-fix-${name}"
      (contentAddressed // {nativeBuildInputs = tools;})
      ''
        cp -R ${src} "$out"
        chmod -R u+w "$out"
        cd "$out"
        ${fix}
      '';
    lanes = {
      # Mirrors the alejandra/statix/deadnix check stages' strictness:
      # `deadnix --edit` without -L deletes an unused lambda pattern name
      # outright, and a call site that still passes the attr surfaces in the
      # eval checks -- exactly the manual migration the check stage's comment
      # prescribes. statix discovers statix.toml at the tree root (the nix
      # lane source carries it). deadnix runs before statix because its
      # edits create statix findings the other order leaves behind: deleting
      # a lambda pattern's last unused name leaves `{}:`, which statix's
      # empty_pattern fix rewrites to `_:`; statix fixes never introduce
      # dead code, so this order converges in one pass.
      nix = mkLane {
        name = "nix";
        tools = [
          pkgs.alejandra
          pkgs.deadnix
          pkgs.statix
        ];
        fix = ''
          deadnix --edit .
          statix fix .
          alejandra --quiet .
        '';
      };
      # The same pinned ruff and selector as the `ruff` check stage -- the
      # fix must not widen or narrow the rule set. `--exit-zero` because
      # findings are this lane's input, not its verdict: an unfixable
      # violation must not fail the lane (the check path still reports it),
      # while a real ruff failure (bad config, panic) still exits 2.
      # `--no-cache` keeps `.ruff_cache` out of the output tree (cwd = $out).
      python = mkLane {
        name = "python";
        tools = [pkgs.ruff];
        fix = ''
          ruff check ${ix.ruffAnnArgs} --fix --exit-zero --no-cache .
        '';
      };
      # `cargo fmt` with the repo-pinned nightly (#3433): the exact
      # toolchain the workspace builds with, plus the rustfmt component the
      # root rust-toolchain.toml's component list omits. cargo-fmt discovers
      # targets via `cargo metadata --no-deps`, which parses manifests only:
      # no Cargo.lock, no network, no dependency sources, so the scoped
      # fileset below suffices. CARGO_HOME points at the build temp dir
      # because cargo wants a writable home even for metadata (cwd is $out,
      # which must stay free of cargo's cache). Formatting is the
      # lane's only stage for now; `clippy --fix` (#3434) slots in BEFORE
      # cargo fmt when it lands, per the formatter-last rule above.
      rust = mkLane {
        name = "rust";
        tools = [rustFmtToolchain];
        fix = ''
          export CARGO_HOME="$TMPDIR/cargo-home"
          cargo fmt --all
        '';
      };
    };

    # Scoped lane inputs: only the files a lane's tools read or rewrite,
    # intersected with the tracked set. An edit outside a lane's fileset
    # leaves that lane's input (hence, content-addressed, its output)
    # untouched -- the whole-tree cache invalidation fix from #3431. The
    # filesets must stay pairwise disjoint: `unite` treats a path emitted by
    # two lanes as a scoping bug. Unlike the check stages' `fd` walks, these
    # include tracked files under hidden directories (.github): a deliberate
    # superset, since hidden-and-tracked is still shipped code.
    sources = let
      tracked = fs.gitTracked paths.root;
      laneSource = fileset:
        fs.toSource {
          inherit (paths) root;
          fileset = fs.intersection tracked fileset;
        };
    in {
      nix = laneSource (
        fs.union
        (fs.fileFilter (file: file.hasExt "nix") paths.root)
        (paths.root + "/statix.toml")
      );
      # `.claude` mirrors the ruff check stage's explicit filter (agent
      # worktrees and assets); ruff.toml rides along so tree-root discovery
      # inside the lane matches a checkout, though the inline flags already
      # carry the whole policy.
      python = laneSource (
        fs.difference
        (
          fs.union
          (fs.fileFilter (file: file.hasExt "py") paths.root)
          (paths.root + "/ruff.toml")
        )
        (fs.maybeMissing (paths.root + "/.claude"))
      );
      # Everything cargo-fmt reads: sources to rewrite plus every Cargo.toml
      # (workspace membership and per-target discovery both come from
      # manifests). rustfmt.toml is maybeMissing because the repo has none
      # today; listing it here means adding one starts scoping the lane
      # instead of being silently ignored. The toolchain pin itself needs no
      # fileset entry: the lane's toolchain is a nativeBuildInput, so a pin
      # bump already rebuilds the lane.
      rust = laneSource (
        fs.unions [
          (fs.fileFilter (file: file.hasExt "rs") paths.root)
          (fs.fileFilter (file: file.name == "Cargo.toml") paths.root)
          (fs.maybeMissing (paths.root + "/rustfmt.toml"))
        ]
      );
    };

    # Union lane outputs into one tree. Lane filesets are disjoint, so the
    # same path arriving from two lanes is a lane-scoping bug, not a merge
    # to resolve: fail loudly rather than let one lane's fix silently shadow
    # another's.
    unite = name: trees:
      pkgs.runCommand name contentAddressed ''
        mkdir -p "$out"
        for tree in ${toString trees}; do
          (cd "$tree" && find . -type f -print0) |
            while IFS= read -r -d "" file; do
              rel="''${file#./}"
              if [ -e "$out/$rel" ]; then
                echo "lane union conflict on $rel (from $tree): lane filesets must be disjoint" >&2
                exit 1
              fi
              mkdir -p "$out/$(dirname "$rel")"
              cp "$tree/$rel" "$out/$rel"
            done
        done
      '';

    fixed = unite "lint-fixed" (lib.mapAttrsToList (name: lane: lane sources.${name}) lanes);

    # The artifact `lint --fix` consumes: one unified diff from the lanes'
    # input snapshot to the fixed tree. Symlinking the trees as `a`/`b`
    # makes the hunk headers `a/<path> b/<path>`, exactly what `git apply`'s
    # default -p1 strips; absolute /nix/store labels would not.
    patch = pkgs.runCommand "lint-fix.patch" contentAddressed ''
      ln -s ${unite "lint-fix-input" (lib.attrValues sources)} a
      ln -s ${fixed} b
      status=0
      diff -ruN a b > "$out" || status=$?
      # 0 = trees identical (empty patch: already clean); 1 = fixes to
      # apply; anything else is a diff failure.
      [ "$status" -le 1 ]
    '';
  in {
    inherit lanes unite fixed patch rustFmtToolchain;
  };

  # `check` is the full CI gate as one repo-owned command: check.yml runs
  # `nix run .#check`, so the same two steps run in CI and locally from a single
  # definition. It targets x86_64-linux explicitly because that is the system CI
  # builds for; a linux runner can only pure-eval the cross-platform darwin
  # images, and that cross-eval was most of what made the old single-threaded
  # `nix flake check` slow. `nix` is taken from the ambient PATH on purpose
  # (this is always invoked as `nix run .#check`, so the host's daemon-matched
  # nix is already present); pinning a client nix here could mismatch the host
  # Nix 2.34.x daemon.
  #
  # Step 1 (nix-fast-build) builds every `ciChecks.x86_64-linux` derivation: it
  # evaluates with nix-eval-jobs (parallel) and streams each drv into a build
  # pool as it resolves. --skip-cached drops paths already in a substituter (a
  # warm run does almost no work), --no-nom keeps plain logs, --no-link leaves no
  # result symlinks. It exits nonzero iff a build or eval fails: that is the gate.
  # --eval-workers 16 with --eval-max-memory-size 6144 is a headroom guard rail
  # (above nix-eval-jobs' 4 GiB default per worker, below the old 8 GiB), not a
  # workaround: the per-crate check split (see the `checks` block below) keeps
  # each worker's eval bounded by the largest single crate. Both binaries are
  # nix-fast-build is the repo-built nixpkgs 1.5.0 package with a patch that
  # makes --skip-cached skip a `local` (warm-store) output, not just a remotely
  # `cached` one. nix-eval-jobs is built against the fleet daemon's stable Nix
  # 2.34 protocol family rather than nixpkgs' moving default. The eval
  # cache is disabled for the parallel evaluator: all workers share one
  # per-flake SQLite database, so writes contend and can fail with "database is
  # busy" without providing useful hits on a fresh commit. See the $fast_build
  # and $eval_jobs comments below.
  #
  # Step 2 (nix-eval-jobs) is the schema/eval gate over the package outputs,
  # broader than the `checks` set step 1 built. nix-eval-jobs is the same
  # parallel evaluator nix-fast-build wraps; run eval-only over
  # packages.x86_64-linux it spreads per-attribute eval across 16 workers and
  # realizes IFD (the `site` import-npm-lock source) on demand. Each worker is a
  # full evaluator that can grow to the 4 GiB-per-worker cap and the host runs
  # many CI jobs at once, so 16 both bounds memory and already collapses the eval
  # toward the slowest single attribute (warm store, eval-cache off: 342s at 1
  # worker, 75s at 16, 70s at 32). The eval cache is off because it is keyed per
  # commit (it never hits on a fresh CI commit) and parallel workers otherwise
  # contend writing the same per-commit sqlite ("database is busy"). The
  # resulting cold per-commit re-eval is tracked separately; a flake eval cache
  # would amortize it. nix-eval-jobs
  # reports a per-attribute eval failure as a JSON `error` line and still exits 0,
  # so the gate is the error-line check; a startup or lock failure exits nonzero
  # and aborts the run (Nushell propagates external failures like bash
  # `set -o pipefail`). Uses the repo-built nix-eval-jobs directly by store path
  # rather than `nix run`.
  #
  # `check required` is the required PR path. It builds the namespaced union of
  # ciChecks and cachePushRoots through one 16-worker evaluator pool, then runs
  # the package schema gate. This replaces two competing self-hosted claims
  # without running two 16-worker clients side by side (which has OOM-killed a
  # 96 GiB runner before). `check closure` remains the manual closure probe.
  check = ix.writeNushellApplication pkgs {
    name = "check";
    meta.description = "Run CI gates: default checks, `required` checks plus publishable closure, or `closure` only";
    text = ''
      # Patched nix-fast-build (packages/nix/nix-fast-build): stock --skip-cached
      # only skips a job whose nix-eval-jobs cacheStatus is `cached` (in a remote
      # substituter); a `local` output (already in this warm runner's store but
      # never pushed) falls through and is re-realized every run. On this CI the
      # rust units and image closures are floating-CA and resolve to `local`, so
      # the patch makes --skip-cached skip `local` too. nixpkgs' 1.5.0 tag is the
      # same commit (7f185e0) the flake ref used to pin, so this is a like-for-like
      # source swap plus the patch. Invoked directly by store path, not `nix run`.
      const fast_build = "${lib.getExe repoPackages.nix-fast-build}"
      # nix-eval-jobs is linked to the stable Nix 2.34 components the fleet
      # daemon runs. Built for x86_64-linux (the CI gate system); `check` itself
      # is x86_64-linux-only.
      const eval_jobs = "${lib.getExe repoPackages.nix-eval-jobs}"

      # Shared build gate: build every derivation under $flake with
      # nix-fast-build and exit 1 on any failure, after replaying each failed
      # build's log. `main` runs it over ciChecks, `main required` over the
      # namespaced union of checks and cache-push roots, and `main closure` over
      # cache-push roots alone.
      def build-gate [flake: string] {
        # ca-derivations: the rust workspace units default to
        # `contentAddressed = true` (lib/rust/cargo-unit.nix), so evaluating
        # the target set resolves floating content-addressed drvs. The
        # evaluator (nix-eval-jobs, which nix-fast-build wraps) needs the
        # `ca-derivations` experimental feature, or it aborts with
        # "experimental Nix feature 'ca-derivations' is disabled". The caller
        # owns cache policy: developers may accept the flake config, while
        # self-hosted CI ignores its restricted cache settings. Pin only the CA
        # feature here so nested evaluator processes remain self-contained.
        # --result-format json --result-file emits one record per attr per phase
        # ({attr, type: EVAL|BUILD, duration, success, error, outputs}) into the
        # cwd. blast-radius consumes this on a later PR via `--timings` to
        # annotate the rebuilt-checks list with wall-clock seconds. The path is
        # relative to the runner cwd; check.yml uploads it as an artifact.
        # nix-fast-build prints "Cannot build <drv>" for a failed check but not the
        # build's own output, so a clippy lint or a test panic surfaces only as a
        # bare "build exited with 1" with no diagnostic to act on. Catch the
        # failure, then replay each failed build's log via `nix log` so the actual
        # clippy/test output lands in the CI log. The failed attrs are read from
        # the --result-file this just wrote (one {attr,type,success,...} record
        # per attr per phase); it is written even on failure.
        # `try` returns false on success and the `catch` returns true, so the
        # failure is carried in an immutable binding (nushell forbids mutating an
        # outer `mut` from inside the catch closure).
        let build_failed = (
          try {
            ^$fast_build ...[
              "--flake" $flake
              # Drive nix-fast-build with the daemon-family-compatible
              # evaluator rather than its nixpkgs default.
              "--nix-eval-jobs" $eval_jobs
              "--eval-max-memory-size" "6144"
              "--eval-workers" "16"
              "--skip-cached"
              # Stop scheduling new checks as soon as one fails (in-flight
              # builds still finish). Default nix-fast-build behavior is to
              # build every remaining check and only report at the end, which
              # spends the full wall time before flake-check goes red (#2128).
              # The failed-attr log replay below still works: the result file
              # is written on failure with the records collected so far.
              "--fail-fast"
              "--no-nom"
              "--no-link"
              "--result-format" "json"
              "--result-file" "check-results.json"
              "--option" "eval-cache" "false"
              "--option" "extra-experimental-features" "ca-derivations"
            ]
            false
          } catch {
            true
          }
        )

        if ("check-results.json" | path exists) {
          let failed = (
            open check-results.json
            | get results
            | where type == "BUILD" and success == false
          )
          for f in $failed {
            # GitHub Actions log group so a long clippy dump stays collapsible;
            # harmless plain text in a local `nix run .#check`.
            print --stderr $"::group::build log: ($f.attr)"
            let inst = $"($flake).($f.attr)"
            # Fast path: replay the retained build log via `nix log` (works for
            # input-addressed checks like the browser smoke test).
            let drv = (
              ^nix eval --raw
                --option extra-experimental-features ca-derivations
                $"($inst).drvPath"
              | complete
            )
            let logged = if $drv.exit_code == 0 and (($drv.stdout | str trim) | is-not-empty) {
              ^nix log ($drv.stdout | str trim) | complete
            } else {
              { exit_code: 1, stdout: "" }
            }
            if $logged.exit_code == 0 and (($logged.stdout | str trim) | is-not-empty) {
              print --stderr $logged.stdout
              # The tail as an annotation too: raw log downloads are blocked
              # from automation, and the checks API only carries annotations.
              let tail = (
                $logged.stdout | lines | last 10 | str join " | " | str substring 0..600
              )
              print $"::error title=($f.attr) build log tail::($tail)"
            } else {
              # A content-addressed build (the rust units default to CA) keeps
              # its log under the *resolved* drv, which `nix log` cannot fetch by
              # the original -- so re-run the one failed check with -L to stream
              # the diagnostic (clippy lint / test output). nix does not cache
              # failures, so this just re-attempts that single check.
              let rebuilt = (do {
                ^nix build ...[
                  $inst
                  "-L"
                  "--no-link"
                  "--option" "extra-experimental-features" "ca-derivations"
                ]
              } | complete)
              print --stderr $rebuilt.stdout
              print --stderr $rebuilt.stderr
              let tail = (
                $"($rebuilt.stdout)\n($rebuilt.stderr)"
                | lines | where {|l| ($l | str trim) | is-not-empty }
                | last 10 | str join " | " | str substring 0..600
              )
              print $"::error title=($f.attr) build log tail::($tail)"
            }
            print --stderr "::endgroup::"
          }
          # One workflow error annotation per failed attr (EVAL and BUILD),
          # carrying the recorded error text. check.yml cats this log to the
          # step's stdout on failure, where the runner parses `::error::`
          # lines into check-run annotations -- the only failure surface
          # reachable when raw log downloads are blocked (annotations ride
          # the checks API). Harmless plain text in a local run.
          let annotated = (
            open check-results.json
            | get results
            | where success == false
          )
          for f in $annotated {
            let err = (
              ($f | get -o error | default "")
              | str replace --all "\n" " | "
              | str substring 0..500
            )
            print $"::error title=($f.attr) ($f.type)::($err)"
          }
        }

        if $build_failed {
          exit 1
        }
      }

      def eval-package-schema [] {
        let tmp = (mktemp --directory --tmpdir "ix-check.XXXXXX")
        let report = ($tmp | path join "flake-schema-eval.jsonl")
        do --capture-errors {
          ^$eval_jobs ...[
            "--flake" ".#packages.x86_64-linux"
            "--workers" "16"
            "--gc-roots-dir" ($tmp | path join "flake-schema-eval-gc")
            "--option" "eval-cache" "false"
            # See the ca-derivations note above: the package set also resolves
            # content-addressed rust units, so this eval needs the feature too.
            "--option" "extra-experimental-features" "ca-derivations"
          ]
        } | tee { save --raw --force $report }

        # nix-eval-jobs exits 0 even when an attribute fails to evaluate, so this
        # error-line check is the gate; a nonzero exit already aborted above. The
        # report is left in place on failure for inspection.
        if (open --raw $report | lines | any {|line| $line | str contains '"error":' }) {
          print --stderr "flake schema evaluation failed; see the error lines above"
          exit 1
        }
        rm --recursive --force $tmp
      }

      def main [] {
        build-gate ".#ciChecks.x86_64-linux"
        eval-package-schema
      }

      # Required PR/merge-group gate. One nix-fast-build invocation evaluates
      # and builds both check roots and publishable closure roots with the same
      # bounded pool; a second package-schema pass retains the broader eval gate.
      def "main required" [] {
        build-gate ".#requiredGateRoots.x86_64-linux"
        eval-package-schema
      }

      # Pre-merge closure gate (closure-gate.yml, #1873): the same build gate
      # over the roots the post-merge cache-push linux lane publishes, darwin
      # cross closure included -- the set #2690 broke while flake-check stayed
      # green (packages are eval-gated only). --skip-cached keeps it
      # O(changed): on the warm-store pool only drvs new relative to main's
      # already-built closure realise.
      def "main closure" [] {
        build-gate ".#cachePushRoots.x86_64-linux"
      }
    '';
  };

  updateMods = ix.writePythonApplication pkgs {
    name = "update-mods";
    src = paths.tools.updateMods;
    pyChecker = "zuban";
    # pydantic validates Modrinth API responses at the boundary so upstream
    # drift fails with a path-precise error rather than a bare KeyError.
    python = pkgs.python314.withPackages (ps: [ps.pydantic]);
    meta.description = "Regenerate Minecraft mod catalogs";
  };

  updateLoaders = ix.writePythonApplication pkgs {
    name = "update-loaders";
    src = paths.tools.updateLoaders;
    pyChecker = "zuban";
    # pydantic validates the PaperMC fill v3 response at the boundary so upstream
    # drift fails with a path-precise error rather than a bare KeyError.
    python = pkgs.python314.withPackages (ps: [ps.pydantic]);
    meta.description = "Refresh Minecraft loader (Paper / Velocity / Fabric) catalogs from upstream";
  };

  ixShellSyncIgnored = ix.writePythonApplication pkgs {
    name = "ix-shell-sync-ignored";
    src = paths.tools.ixShellSyncIgnored;
    pyChecker = "zuban";
    runtimeInputs = [
      pkgs.git
      pkgs.gnutar
    ];
    meta.description = "Copy git-ignored files into an ix shell workspace";
  };

  # `nix run .#cve-scan`: scan the whole Nix closure of the repo's key outputs
  # One symlink-free directory holding every skill under `skills/`, ready to
  # copy into `.claude/skills`.
  skillsDir = ix.skills.mkSkillsDir {inherit pkgs;};

  # The `index` Claude Code plugin: every index skill bundled for `--plugin-dir`,
  # invoked as `/index:<skill>`. This is the pure-index default (no hooks, no
  # personal skills); a consumer wanting extras calls `ix.claudePlugin.mkPlugin`
  # with `extraSkills`/`hooks` directly.
  claudePluginDir = ix.claudePlugin.mkPlugin {
    inherit pkgs;
    name = "index";
  };

  # Declarative subagents rendered to a symlink-free `.claude/agents` directory.
  # Keep this outside the Claude plugin: plugins namespace subagent names, but
  # hooks and skills call these by bare `subagent_type` (`code-reviewer`, etc.).
  agentDefinitions = import (paths.packagesRoot + "/agent/subagents.nix") {
    inherit
      ix
      lib
      repoPackages
      ;
  };
  agentsDir = ix.agents.mkAgentsDir {
    inherit pkgs;
    agents = agentDefinitions.renderedAgents;
    inherit (agentDefinitions) rawFiles;
  };

  mcSource = ix.writeNushellApplication pkgs {
    name = "mc-source";
    text = builtins.readFile paths.tools.mcSource;
    runtimeInputs = [
      (pkgs.callPackage packageRegistry.byId.vineflower.path {inherit ix;})
    ];
    meta.description = "Decompile a Minecraft server jar with Mojang mappings via Vineflower";
  };

  updateSounds = ix.writeNushellApplication pkgs {
    name = "update-sounds";
    text = builtins.readFile paths.tools.updateSounds;
    meta.description = "Refresh the pinned Minecraft sound pack in packages/minecraft/sound";
  };

  # The indexbench CLI built for this system, fed to `mkBenchSuite` and the
  # `apps.bench` perf job. Also surfaced as `packages.indexbench` through the
  # registry; this binding just avoids re-resolving the package set here.
  inherit (repoPackages) indexbench;

  # The reproducible alloc-count bench binary from the shared workspace graph.
  # It installs the counting allocator and prints an `@bench name=allocations`
  # line, so its metric is deterministic and gateable as a flake check.
  indexbenchAllocDemo = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "indexbench-alloc-demo";
    packageName = "indexbench";
    includeTestCases = false;
    meta.mainProgram = "indexbench-alloc-demo";
  };

  # The repo's own demonstration suite: a trivial macro command, run through the
  # framework end to end. `nix run .#bench` invokes this perf job. Consumers add
  # their own suites the same way via `ix.mkBenchSuite`. The `allocCheck` wires
  # the reproducible alloc-count bench into a flake check.
  indexbenchSelfDemo = ix.mkBenchSuite pkgs {
    name = "self-demo";
    inherit indexbench;
    macros = [
      {
        name = "true";
        command = "true";
      }
    ];
    allocCheck = {
      bench = lib.getExe indexbenchAllocDemo;
      # The demo makes exactly 64 heap allocations by construction (see
      # packages/indexbench/src/bin/alloc-demo.rs), so this budget is an exact,
      # toolchain-stable constant; any added allocation trips the gate.
      budgets.allocations = 64;
    };
  };

  # `paths.site` is the git-filtered `site` subtree input (a store copy, not
  # a local path), so `lib.fileset`/`gitTracked` cannot apply to it; the input
  # already scopes source identity to the subtree.
  siteSrc = paths.site;

  siteTests = ix.buildNpmVitest pkgs {
    pname = "ix-site";
    version = "0.1.0";
    src = siteSrc;
    preTest = ''
      node node_modules/@sveltejs/kit/src/cli.js sync
    '';
  };

  repoPackages = ix.packageSetFor pkgs;
  inherit (repoPackages) site;

  # Since the jj megamerge migration the fork inputs already ARE the patched
  # trees, so the old `patched-src-<name>` / `patch-dag-<name>` check family
  # is gone (ix still builds it for its own patch-dir forks via
  # `ix.mkForkChecks`). The one survivor is clippy: its `derivedPatches`
  # generators still run at build time, so this check proves they apply to
  # the fetched megamerge tree without building all of llm-clippy.
  forkChecks = {
    patched-src-clippy = (ix.patchedSrcFor pkgs) {
      name = "clippy";
      src = ix.clippySrc;
    };
  };

  # One general updater for every content source in the repo, run in parallel
  # via dag-runner (the same engine `lint` uses). The Minecraft catalog and
  # sound updaters are fixed apps; the pinned prebuilt-binary updaters
  # (claude-code, yc, ...) are discovered from the registry `updateScript` flag,
  # so adding such a package joins this set with no change here. The nodes are
  # independent (each writes its own source files: mod/loader/sound catalogs or
  # packages/<id>/manifest.json), so they run concurrently. dag-runner fails the
  # run if any node exits non-zero, so a bad signature or fetch error surfaces
  # in CI. Each updater writes relative to the repo root, so `update` must run
  # from the repo root.
  updatableEntries = packageRegistry.updateScriptEntriesFor system;
  updaterFor = entry: let
    pkg =
      lib.attrByPath entry.packageSet.attrPath
      (throw "update: package `${entry.id}` is flagged `updateScript = true` but is absent from the package set for ${system}")
      repoPackages;
  in
    lib.getExe (
      pkg.updateScript
        or (throw "update: package `${entry.id}` is flagged `updateScript = true` but exposes no `passthru.updateScript`")
    );
  updateNodes =
    {
      mods.command = [(lib.getExe updateMods)];
      loaders.command = [(lib.getExe updateLoaders)];
      sounds.command = [(lib.getExe updateSounds)];
    }
    // lib.genAttrs' updatableEntries (
      entry: lib.nameValuePair entry.id {command = [(updaterFor entry)];}
    );
  updateSpec = (pkgs.formats.json {}).generate "update-dag.json" {nodes = updateNodes;};
  # Machine-readable registry view for update.yml's "Build changed packages"
  # step: repo-relative package directory -> the flake attr that builds it on
  # this system. The workflow maps each file the updaters changed to its owning
  # package through this table instead of guessing an attr from path segments,
  # which breaks for nested catalog manifests (#2036). Restricted to entries
  # with a `flake` target enabled here, so a platform-gated updater (dia is
  # aarch64-darwin-only) is absent from the Linux map and gets skipped rather
  # than built as a missing attr.
  updatablePackages = lib.genAttrs' (
    lib.filter (entry: entry.updateScript) (packageRegistry.flakeEntriesFor system)
  ) (entry: lib.nameValuePair "packages/${entry.relativePath}" entry.flake.attrName);
  update = ix.writeNushellApplication pkgs {
    name = "update";
    meta.description = "Refresh every repo content source (Minecraft catalogs + pinned binaries) in parallel via dag-runner";
    runtimeInputs = [repoPackages.dag-runner];
    text = ''
      # nu
      def --wrapped main [...args] {
        exec dag-runner ...$args ${updateSpec}
      }
    '';
  };

  # Cross-compiled standalone packages, exposed as
  # `packages.<host>.<attr>-<triple>` and optionally aliased into native Darwin
  # package namespaces by flake.nix. Linux-only: the Apple (zig + macOS SDK) and
  # Rust target graph run on a Linux build host; Darwin hosts build native
  # packages directly and cannot host this Linux→Darwin lane. Package definitions
  # stay target-agnostic: the cross lane swaps the `ix.rustWorkspace.units`
  # handle underneath them instead of passing a separate cross API.
  darwinTargetsBySystem = {
    aarch64-darwin = "aarch64-apple-darwin";
    x86_64-darwin = "x86_64-apple-darwin";
  };
  targetSystemFor = target:
    if lib.hasSuffix "-apple-darwin" target
    then
      if lib.hasPrefix "aarch64-" target
      then "aarch64-darwin"
      else "x86_64-darwin"
    else throw "cross: unsupported target `${target}`";
  crossEntries = packageRegistry.crossEntriesFor system;
  crossWorkspace = ix.rustWorkspaceFor pkgs;
  # One nixpkgs cross scope per darwin target, shared by every cross entry
  # that builds through upstream nixpkgs packaging (nix-ix) rather than the
  # cargo-unit lane. Lazy: rust-only cross entries never force the
  # instantiation, so the scope costs nothing until a C/C++ cross package
  # is actually evaluated.
  crossNixpkgsByTarget = lib.genAttrs (lib.attrValues darwinTargetsBySystem) (
    target: ix.darwinCrossPkgs pkgs target
  );
  crossIxFor = target: let
    targetWorkspace =
      crossWorkspace
      // {
        units = crossWorkspace.unitsFor {inherit target;};
      };
  in
    ix
    // {
      inherit pkgs;
      cargoUnit = ix.cargoUnitFor pkgs;
      rustWorkspace = targetWorkspace;
      cross = {
        isCross = true;
        inherit target;
        targetSystem = targetSystemFor target;
        # nixpkgs' own cross scope for packages that build through upstream
        # packaging rather than the rust unit graph (nix-ix's modular C++
        # closure). See lib/darwin/nixpkgs-cross.nix.
        pkgs = crossNixpkgsByTarget.${target};
      };
      wrapPackage = wrapperPkgs: args: ix.wrapPackage wrapperPkgs (args // {isCross = true;});
    };
  buildCrossPackage = target: entry:
    lib.callPackageWith (
      pkgs
      // {
        inherit entry repoPackages;
        ix = crossIxFor target;
        writeNushellApplication = ix.writeNushellApplication pkgs;
        updateScriptWriter = ix.writeNushellApplication pkgs;
      }
    )
    entry.path {};
  crossPackages = lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux (
    lib.listToAttrs (
      lib.concatMap (
        entry:
          map (
            target: lib.nameValuePair "${entry.cross.attrName}-${target}" (buildCrossPackage target entry)
          )
          entry.cross.targets
      )
      crossEntries
    )
  );
  # The eval-time IFD closure of each cross target's unit graph. A Mac cannot
  # *build* a Linux→Darwin cross output, but the Darwin package aliases force it
  # to *evaluate* the cross derivation, and that eval imports the rendered
  # `cargo-units.nix` (which is generated from `cargo-unit-graph.json`, itself
  # generated from the vendor dir). Those three are build-time deps of the cross
  # outputs, so `attic push` of the outputs' *runtime* closures never carries
  # them (RFC 0009's substitute-or-nothing trap: #1687). Publishing them lets a
  # Mac substitute the IFD outputs instead of trying to build x86_64-linux drvs
  # at eval; because these are input-addressed drvs, their eval-time out paths
  # are known, so cache-push's probe sees the same paths a Mac's eval demands.
  # Keyed by distinct cross target (the unit graph is shared per target, not per
  # package), derived from `crossEntries` so a new cross target or entry joins
  # this set with no hand-kept list. Same Linux-host gate as `crossPackages`:
  # the cross graphs only build on the Linux host that owns the cross lane.
  crossIfdRoots = lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux (
    let
      crossTargets = lib.unique (lib.concatMap (entry: entry.cross.targets) crossEntries);
      rootsForTarget = target: let
        units = crossWorkspace.unitsFor {inherit target;};
      in
        # These three ARE the whole eval-time closure: the `import unitsNix`
        # forces `unitsNix`, which references only `unitGraphJson` and `vendorDir`
        # (the cargo-lock it also reads is a plain flake source path, always
        # present). `cargo-vendor-config.toml` is not a fourth root: it is a
        # build input of the `unitGraphJson` builder, not on the import path and
        # not in `vendorDir`'s closure, so substituting `unitGraphJson`'s output
        # makes it moot -- the Mac never runs that builder.
        {
          "cross-ifd-${target}-units-nix" = units.unitsNix;
          "cross-ifd-${target}-unit-graph" = units.unitGraphJson;
          "cross-ifd-${target}-vendor-dir" = units.vendorDir;
        };
    in
      lib.mergeAttrsList (map rootsForTarget crossTargets)
  );
  # A cross package whose build rides a distinct `cargoUnit.buildWorkspace`
  # instead of the shared `crossWorkspace` (codex: its codex-rs is a second
  # workspace) exposes that workspace's unit-graph IFD artifacts via
  # `passthru.workspaceIfdRoots`. `crossIfdRoots` only covers the shared
  # workspace, so harvest these too -- otherwise a Mac consumer substituting the
  # cross output re-vendors/re-renders that graph at eval and hits the #1890
  # trap on x86_64-linux drvs it cannot build. Generic over `crossPackages`, so
  # a future second-workspace cross package joins with no hand-kept list.
  crossPackageIfdRoots = lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux (
    lib.concatMapAttrs (
      name: pkg:
        lib.mapAttrs' (
          rootName: drv: lib.nameValuePair "cross-ifd-${name}-${rootName}" drv
        )
        (pkg.passthru.workspaceIfdRoots or {})
    )
    crossPackages
  );
  darwinPackageAliases = lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux (
    lib.genAttrs (lib.attrNames darwinTargetsBySystem) (
      darwinSystem: let
        target = darwinTargetsBySystem.${darwinSystem};
      in
        lib.listToAttrs (
          lib.concatMap (
            entry:
              lib.optional (entry.cross.exposeNativeDarwin && builtins.elem target entry.cross.targets) (
                lib.nameValuePair entry.cross.attrName crossPackages."${entry.cross.attrName}-${target}"
              )
          )
          crossEntries
        )
    )
  );

  repoFlakePackages = lib.genAttrs' (packageRegistry.flakeEntriesFor system) (
    entry:
      lib.nameValuePair entry.flake.attrName (
        lib.attrByPath entry.packageSet.attrPath
        (throw "packages/${entry.relativePath}/package.nix: flake output `${entry.flake.attrName}` needs packageSet.attrPath")
        repoPackages
      )
  );

  rustPackageTestSets = let
    cargoUnit = ix.cargoUnitFor pkgs;
    rustWorkspace = ix.rustWorkspaceFor pkgs;
    # A crate with a `packageSet` is built through `repoPackages` and carries
    # its own `passthru.tests`. A lib-only workspace crate has no `packageSet`
    # and is not in `repoPackages`, so select its library straight from the
    # shared unit graph (same path ix-vt's default.nix uses). The unit graph is
    # keyed by the Cargo package name (library unit key: dashes underscored),
    # which the registry id need not match (packages/usage/core is id
    # `usage-core`, crate `ix-usage-core`), so read the name from the crate's
    # own manifest.
    packageTestsFor = entry:
      if entry.packageSet != null
      then
        (
          lib.attrByPath entry.packageSet.attrPath
          (throw "packages/${entry.relativePath}/package.nix: passthruTests needs packageSet.attrPath")
          repoPackages
        ).passthru.tests or {
        }
      else let
        cargoPackageName = (lib.importTOML (entry.path + "/Cargo.toml")).package.name;
      in
        (cargoUnit.selectLibraryWithTests rustWorkspace.units {
          library = lib.replaceStrings ["-"] ["_"] cargoPackageName;
          packageName = cargoPackageName;
        }).passthru.tests or {
        };
    # Two keyings of the same leaf test derivations:
    #
    #  * `flat` keys each per-#[test] check as its own top-level name
    #    (`<prefix>-<target>-tests-<case>`). This is what the public `checks`
    #    output needs: the flake schema requires every `checks.<system>.<name>`
    #    to be a derivation, so a nested attrset there fails `nix flake check`.
    #
    #  * `sharded` nests each package's checks under one `recurseForDerivations`
    #    attr (`<prefix>.<target>-tests-<case>`). This is what the memory-bounded
    #    CI evaluator (nix-fast-build / nix-eval-jobs / blast-radius) consumes
    #    through the separate `ciChecks` output.
    #
    # Why the sharded shape exists: nix-eval-jobs hands the root attrpath to one
    # worker and forces its child names to recurse. With the flat set, that one
    # worker forces every crate's per-#[test] manifest IFD at once and balloons
    # to tens of GiB, which earlyoom kills on the shared CI host. The nested
    # shape makes the root return cheap per-package names and forces each
    # crate's manifests inside its own worker job, which restarts at the memory
    # cap between packages (ENG-2201). The nested value must stay a thunk:
    # filtering empties (e.g. `tests != {}`) would force every manifest during
    # enumeration and reintroduce the balloon, so empty groups are left in.
    flatPackageChecks = prefix: tests: lib.mapAttrs' (n: t: lib.nameValuePair "${prefix}-${n}" t) tests;
    shardedPackageChecks = prefix: tests: {
      ${prefix} =
        tests
        // {
          recurseForDerivations = true;
        };
    };
    repoEntries = packageRegistry.passthruTestEntriesFor system;
    moduleRustPackages = {
      resource-monitor-stats-writer = cargoUnit.selectBinaryWithTests rustWorkspace.units {
        binary = "resource-monitor-stats-writer";
      };
    };
    # cargoAudit scans the single workspace Cargo.lock against the advisory DB,
    # so it is one lockfile-scoped check (it rebuilds only on a Cargo.lock
    # change, never on a source edit) rather than a per-crate gate. Expose it
    # once instead of aliasing the same derivation onto every crate.
    workspaceAuditTests = lib.optionalAttrs (rustWorkspace.units.policyChecks ? cargoAudit) {
      rust-cargoAudit = rustWorkspace.units.policyChecks.cargoAudit;
    };
    collectRust = group:
      lib.mergeAttrsList (
        map (entry: group entry.passthruTests.prefix (packageTestsFor entry)) repoEntries
        ++ lib.mapAttrsToList (
          packageName: package: group "rust-${packageName}" (package.passthru.tests or {})
        )
        moduleRustPackages
      )
      // workspaceAuditTests;
  in {
    flat = collectRust flatPackageChecks;
    sharded = collectRust shardedPackageChecks;
  };

  # The repo's inline script checks, split out to live next to what they
  # check (#3898). Every file takes only what its checks actually read and
  # builds through the one shared script-check shape (lib/checks.nix), so
  # the success-marker boilerplate exists in exactly one place. Sources are
  # scoped filesets; the whole-tree exception (the lint gate) is documented
  # at its binding in lib/dev/checks.nix.
  mkCheck = (import ./checks.nix {inherit lib;}).mkScriptCheck {
    inherit pkgs;
    prefix = "check";
  };
  repoPolicyChecks = import ./dev/checks.nix {
    inherit lib pkgs paths mkCheck lint lintStage lintFix;
    inherit (ix) ruffAnnArgs;
  };
  libUtilChecks = import ./util/checks.nix {
    inherit pkgs mkCheck;
    inherit (ix) formatProvenance artifacts;
  };
  libServiceChecks = import ./services/checks.nix {
    inherit pkgs mkCheck;
    inherit (ix.mutableJson) mergeProgram;
  };
  crossDarwinChecks = import ./darwin/cross-checks.nix {
    inherit pkgs mkCheck crossPackages;
  };
  astlogRuleChecks = import (paths.root + "/astlog-rules/checks.nix") {
    inherit lib pkgs paths mkCheck;
    inherit (repoPackages) astlog;
  };
  agentSurfaceChecks = import (paths.packagesRoot + "/agent/checks.nix") {
    inherit mkCheck skillsDir agentsDir;
  };
  blastRadiusChecks = import (paths.packagesRoot + "/blast-radius/checks.nix") {
    inherit lib pkgs paths mkCheck;
  };
  netTraceChecks = import (paths.packagesRoot + "/net-trace/checks.nix") {
    inherit lib pkgs paths mkCheck;
  };
  scipqlChecks = import (paths.packagesRoot + "/code/scipql/checks.nix") {
    inherit mkCheck;
    inherit (repoPackages) scipql;
  };
  personalConfigChecks = import (paths.users + "/andrewgazelka/checks.nix") {
    inherit lib pkgs ix paths mkCheck;
    inherit (repoPackages) nushell;
  };

  # Fork-syntax island: tests/ may use fork-only syntax (underscore digit
  # separators), so a stock-Nix evaluator must hit the gate's install
  # message when a tests-derived check is forced, not a bare parse error
  # (index#3635).
  tests = ix.evaluatorGate.require "tests" (import paths.tests {
    inherit
      nixpkgs
      ix
      paths
      home-manager
      ;
  });

  examples = ix.examplesFor {hostSystem = system;};
  exampleFleets = examples.fleets;
  # Every example's VMs (fleet-shaped or plain multi-VM), the coverage
  # surface for cache-push and security roots: multi-VM examples carry no
  # lifecycle plan (ix#8306) but their closures must stay eval- and
  # build-covered.
  exampleVms = examples.vms;

  # Same fleets with "health-check-" prepended to every external name, so the
  # lifecycle scripts that force-delete VMs by name can never clobber an
  # unrelated production VM that happens to share the example's node name
  # (`nginx`, `factions`, ...). `withNodePrefix` only rewrites plan data, so
  # both surfaces share one NixOS closure evaluation per node instead of
  # evaluating every example fleet twice (ENG-2411).
  healthCheckExampleFleets =
    lib.mapAttrs (
      _name: fleet: fleet.withNodePrefix "health-check-"
    )
    exampleFleets;

  # Surface every example's `ix fleet <sub>` wrapper as a flake package.
  # Each example contributes `packages.<system>.<example>-{up,health,...}`,
  # which lets `nix run .#nginx-lifecycle-up` invoke the existing fleet
  # plumbing through the wrapper's `meta.mainProgram`, and
  # `nix build .#nginx-lifecycle-up` produce the wrapper script on disk.
  examplePackages = let
    fleetSubs = [
      "up"
      "health"
      "status"
      "logs"
      "replace"
      "switch"
      "diff"
    ];
  in
    lib.concatMapAttrs (
      name: fleet:
        lib.genAttrs' fleetSubs (sub: {
          name = "${name}-${sub}";
          value = fleet.${sub}.overrideAttrs (old: {
            meta =
              (old.meta or {})
              // {
                description = "Run `ix fleet ${sub}` against the ${name} example fleet";
              };
          });
        })
    )
    exampleFleets;

  healthChecks =
    import ./image/health-checks.nix
    {
      inherit lib pkgs;
      inherit (ix) kdl writeNushellApplication;
      dagRunner = repoPackages.dag-runner;
    }
    {
      exampleFleets = healthCheckExampleFleets;
      exampleNames = lib.attrNames exampleFleets;
    };

  baseImage = ix.mkImage {
    modules = [(paths.root + "/images/system/base")];
  };

  vcfsGuestEvalImage = ix.mkImage {
    modules = [(paths.root + "/images/system/vcfs-guest-eval")];
  };

  # Non-NixOS OCI example images (ubuntu, debian, ...). They live under
  # `examples/oci` with the same hierarchical shape as the VM examples, but
  # return images instead of VM results and are exposed as opt-in packages only.
  nonNixExampleImages =
    lib.mapAttrs'
    (
      name: entry:
        lib.nameValuePair "non-nix-${name}" (
          import (entry.path + "/default.ix") {
            index = {
              lib = ix;
            };
          }
        )
    )
    (
      ix.discoverTree {
        root = paths.examples + "/oci";
        requiredFiles = ["default.ix"];
      }
    );

  # The content-addressed `image.json` for each non-Nix example, surfaced as its
  # own package so the small artifact is buildable directly (`nix build
  # .#non-nix-ubuntu-description`) and cached independently of the materialized
  # tar it regenerates. See #679.
  nonNixExampleDescriptions =
    lib.mapAttrs' (
      name: image: lib.nameValuePair "${name}-description" image.passthru.description
    )
    nonNixExampleImages;

  # Build the check catalog from a rust-package keying. `checks` (flat: one
  # derivation per `checks.<system>.<name>`, required by the flake schema and
  # `nix flake check`) and `ciChecks` (sharded: one `recurseForDerivations` group
  # per package, what the memory-bounded CI evaluator consumes) share the same
  # explicit checks; only the rust keying differs (ENG-2201). The
  # collision guard runs per keying, so producing `ciChecks` only forces the
  # cheap per-package names, never the flat per-#[test] spine.
  catalogFor = rustPackageSet:
    lib.optionalAttrs (system == ix.system) (
      let
        rustChecks =
          {
            cargo-unit-real-workspaces = tests.cargoUnitRealWorkspaces;
            cargo-unit-prebuilt-library = tests.cargoUnitPrebuiltLibrary;
            sdk-rust-prebuilt = tests.sdkRustPrebuilt;
          }
          // rustPackageSet;
        explicitChecks =
          {
            inherit (tests) eval;
            # Boots a NixOS VM running the minecraft-blocks producer's Paper
            # server and asserts the BlockEvents plugin's onEnable succeeded
            # with no exception (ENG-2186). Paper's paperclip bootstrap is
            # pre-run at build time so the VM never needs the network; see
            # tests/minecraft-blocks-vm.nix.
            minecraft-blocks-vm = tests.minecraftBlocksVm;
            # Boots a NixOS VM running the Minestom spleef example server under
            # `services.minestom` and asserts it serves the Minecraft protocol
            # (readiness log line, open port, real server-list ping); see
            # tests/minestom-spleef-vm.nix.
            minestom-spleef-vm = tests.minestomSpleefVm;
            # Builds the base OCI archive and asserts its baked nix store DB
            # registers the pinned nixpkgs source as valid, so a fresh VM's first
            # `nix` command does not re-copy the tree through VCFS (ix
            # #1748/#1749/#1815). Its own check because it builds an image.
            base-image-nix-db = tests.baseImageNixDb;
            # Holds the `nix.registry.index.to` construction against both
            # shapes of `self` (narHash-bearing git consumption vs the
            # path-locked submodule seam), the boundary that broke twice on
            # 2026-07-22 (index#3981, fixed in #3988). Pure eval; its own
            # check so a regression fails by name, not inside `eval`.
            image-registry-pin = tests.imageRegistryPin;
            # The commented knob/env reference at the Home Manager consumption
            # site (index#3710) is asserted, at eval, against what the
            # claude-code wrapper actually accepts (functionArgs plus
            # passthru.knobDefaults) and against the pinned CLI version, so
            # the reference cannot silently go stale.
            claude-code-knob-reference = import (paths.packagesRoot + "/agent/claude-code/knob-reference-check.nix") {
              inherit lib pkgs;
              claudeCode = repoPackages.claude-code;
              hmModule = paths.packagesRoot + "/agent/home-manager/claude-code.nix";
            };
            run-records-session = repoPackages.run.passthru.tests.recordsSession;
            # hive's quality lane through the same shared ix.buildElixirCheck:
            # `mix compile --warnings-as-errors` (Elixir 1.18's set-theoretic type
            # checker) plus format, `mix credo --strict`, and test. The lint half
            # is also astlog-rules/elixir.astlog. See
            # packages/andrewgazelka/hive/default.nix.
            hive-elixir = repoPackages.hive.passthru.tests.elixir;
            # Deterministic alloc-count gate for indexbench: runs the counting-
            # allocator demo bench once through `indexbench assert` and fails if its
            # allocation count exceeds the declared budget. Reproducible, unlike
            # timing/RSS, so it earns a flake check; the timing/RSS perf job lives
            # under `apps.bench` instead.
            indexbench-self-demo-alloc = indexbenchSelfDemo.check;
            site-test = siteTests.all;
          }
          // agentSurfaceChecks
          // astlogRuleChecks
          // blastRadiusChecks
          // crossDarwinChecks
          // netTraceChecks
          // libServiceChecks
          // libUtilChecks
          // personalConfigChecks
          // repoPolicyChecks
          // scipqlChecks;
        checkNameCollisions = lib.intersectLists (lib.attrNames explicitChecks) (lib.attrNames rustChecks);
      in
        assert lib.assertMsg (checkNameCollisions == [])
        "checks: duplicate names across explicit/rust sets: ${lib.concatStringsSep ", " checkNameCollisions}";
          explicitChecks // rustChecks
    );
  packageSet =
    lib.optionalAttrs (system == ix.system) {
      base = baseImage;
      vcfs-guest-eval = vcfsGuestEvalImage;
    }
    // {
      health-checks = healthChecks.dag;
      health-checks-zellij = healthChecks.zellij;
      inherit lint site;
      # Fixer-lane outputs (#3432): `lint-fixed` is the union of the lanes'
      # fixed trees; `lint-fix-patch` is diff(snapshot, fixed), the artifact
      # `nix run .#lint -- --fix` builds and git-applies. An empty patch
      # means the tree is already clean.
      lint-fixed = lintFix.fixed;
      lint-fix-patch = lintFix.patch;
      site-dev = site.passthru.devServer;
      # Embedding duplicate-code finder CLI (index#3905): the ix-mcp kernel's
      # bundled `embed` module run as `python -m embed` on the same pinned
      # interpreter, so `nix run .#embed -- dupes . --k 40 --json` sees the
      # same torch/MPS runtime and parquet cache as in-kernel `import embed`.
      embed = repoPackages.mcp.passthru.embedCli;
      update-mods = updateMods;
      update-loaders = updateLoaders;
      inherit update;
      ix-shell-sync-ignored = ixShellSyncIgnored;
      mc-source = mcSource;
      update-sounds = updateSounds;
      agents = agentsDir;
      skills = skillsDir;
      claude-plugin = claudePluginDir;
      # CI tools are pinned to the flake's nixpkgs so workflows resolve exact
      # executables with `nix build .#<tool>` instead of trusting runner PATH.
      # cache-push uses attic/jq/xargs/gh; cve-scan uses curl/jq/tar, and its
      # PR gate uses node for ratchet-cli.mjs.
      # This avoids depending on a tool being on the runner PATH or a floating
      # `nixpkgs#` registry reference. The self-hosted runner PATH carries
      # coreutils + nix but not findutils, jq, gh, or node, so the bare
      # commands are `command not found` (cve-scan run 28598889924 died on
      # exactly that; the regression gate's ratchet step died the same way on
      # bare `node` in run 29196909666).
      inherit
        (pkgs)
        attic-client
        coreutils
        curl
        jq
        findutils
        gh
        gnutar
        nodejs
        ;
    }
    // lib.optionalAttrs (system == "x86_64-linux") {inherit check;}
    // repoFlakePackages
    // examplePackages
    // nonNixExampleImages
    // nonNixExampleDescriptions
    // crossPackages
    // healthChecks.lifecyclePackages;
  securityRootRegistry = let
    mkRoot = ix.securityRoots.mkRoot;
    owner = "indexable-inc/index";
    cachePolicy = {
      inherit owner;
      class = "cache-only";
      environment = "none";
      exposure = "none";
      criticality = "low";
      slaHours = 168;
    };
    baseImagePolicy = {
      inherit owner;
      class = "base-image";
      environment = "development";
      exposure = "internal";
      criticality = "medium";
      slaHours = 72;
    };
    # Business exposure is never inferred from package metadata. Add a complete
    # policy here only when a package is known to be deployed or distributed;
    # every unspecified non-image output remains cache hygiene, not exposure.
    securityRootPolicies = {};
    packageEntries =
      lib.mapAttrs (
        name: package: let
          isImage = package ? passthru.toplevel;
          path = package.passthru.toplevel or (lib.getOutput package.outputName package);
          policy =
            if isImage
            then baseImagePolicy
            else securityRootPolicies.${name} or cachePolicy;
        in {
          inherit path;
          root = mkRoot (
            {
              attr = "packages.${system}.${name}";
              inherit name;
            }
            // policy
          );
        }
      )
      packageSet;
    exampleEntries =
      lib.concatMapAttrs (
        exampleName: example:
          lib.mapAttrs' (
            node: path: let
              name = "example-${exampleName}-${node}";
            in
              lib.nameValuePair name {
                inherit path;
                root = mkRoot {
                  attr = "examplesFor.${system}.vms.${exampleName}.systemPackages.${node}";
                  inherit name owner;
                  class = "deployed-service";
                  environment = "development";
                  exposure = "internal";
                  criticality = "medium";
                  slaHours = 72;
                };
              }
          )
          example.systemPackages
      )
      exampleVms;
    entries =
      if pkgs.stdenv.hostPlatform.isDarwin
      then packageEntries
      else packageEntries // exampleEntries;
  in {
    securityRoots = lib.mapAttrs (_: entry: entry.root) entries;
    securityRootPaths = lib.mapAttrs (_: entry: entry.path) entries;
  };
in {
  packages = packageSet;

  # Non-schema output consumed by update.yml via `nix eval --json`; see the
  # binding above for what it maps.
  inherit updatablePackages;

  # CI-only push roots for cache-push.yml. Two adjustments to `packages` keep the
  # cache useful to `ix apply` while cutting the monolithic `*-oci.tar` archives that
  # dominate the run -- each is one uncompressed blob that never dedups, cold
  # every run since check.yml only eval-validates packages:
  #
  #   1. Every NixOS image is replaced by its `toplevel` closure -- the artifact
  #      `ix apply` substitutes (consumers reconstruct the archive on demand via
  #      streamLayeredImage). Non-image packages, and non-NixOS OCI images (which
  #      expose no `toplevel`), pass through unchanged. See lib/image/oci-layer.nix.
  #   2. The `health-check-*` packages (and the `health-checks{,-zellij}` runners)
  #      pin every fleet node's `toplevel` closure as a build dep
  #      (lib/image/health-checks.nix). Drop the wrapper scripts and add the
  #      fleet node `toplevel` closures directly, so the closures those checks
  #      drag in stay cached without pushing the per-fleet script derivations.
  #   3. The cross lane's eval-time IFD outputs (`crossIfdRoots`): the rendered
  #      `cargo-units.nix`, its `cargo-unit-graph.json`, and the vendor dir a Mac
  #      forces at eval when it substitutes a Darwin cross output. These are
  #      build-time deps of the cross packages, so they are absent from those
  #      packages' runtime closures; adding them as roots is the fix for #1687.
  #      `crossPackageIfdRoots` extends this to cross packages that ride a second
  #      `buildWorkspace` (codex's codex-rs), whose own unit graph `crossIfdRoots`
  #      -- keyed off the shared `crossWorkspace` -- does not see.
  #   4. On Darwin hosts, the native lane's eval-time IFD outputs
  #      (`nativeIfdRoots`): the same three unit-graph artifacts as (3) but for
  #      the host's own target, which a Darwin consumer forces at eval when it
  #      evaluates any native wrapper (codex, claude-code) against the workspace
  #      unit graph. Runtime closures never carry them, so without explicit
  #      roots every Darwin consumer re-vendors and re-renders the graph at
  #      eval -- the same trap as (3), for the darwin cache lane (#1890).
  cachePushRoots = let
    # Per-node `health-check-*` lifecycle packages and the two
    # `health-checks{,-zellij}` runners all share the `health-check` prefix.
    isHealthCheck = lib.hasPrefix "health-check";
    imagesAsClosures = lib.mapAttrs (_: p: p.passthru.toplevel or p) (
      lib.filterAttrs (name: _: !isHealthCheck name) packageSet
    );
    # `systemPackages` keys each VM's toplevel as `<vm>-system`; the
    # example-name prefix keeps VMs sharing a name across examples distinct.
    exampleNodeToplevels =
      lib.concatMapAttrs (
        exampleName: example:
          lib.mapAttrs' (
            node: toplevel: lib.nameValuePair "${exampleName}-${node}" toplevel
          )
          example.systemPackages
      )
      exampleVms;
    # Native analog of `crossIfdRoots` (adjustment 4). `crossWorkspace` with no
    # target override IS the host workspace, so these are exactly the drvs a
    # Darwin consumer's eval of the native wrappers imports.
    nativeIfdRoots = lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
      native-ifd-units-nix = crossWorkspace.units.unitsNix;
      native-ifd-unit-graph = crossWorkspace.units.unitGraphJson;
      native-ifd-vendor-dir = crossWorkspace.units.vendorDir;
    };
  in
    # Fleet node toplevels are NixOS closures: on Darwin they can only
    # eval-error (every `<fleet>-<node>` row in the first darwin lane run was
    # an eval failure, run 28762717645), so they stay a linux-lane concern.
    # Alias-shadowed natives (dag-runner, nix-web-monitor) need no exclusion
    # here: the flake grafts `linuxDarwinAliases` over this set, so the darwin
    # lane sees the cross drvs and its system filter drops them.
    if pkgs.stdenv.hostPlatform.isDarwin
    then imagesAsClosures // nativeIfdRoots
    else imagesAsClosures // exampleNodeToplevels // crossIfdRoots // crossPackageIfdRoots;

  # The policy manifest is safe to `nix eval --json`: derivations live in the
  # separate securityRootPaths output and must be realized before their terminal
  # store paths are trusted.
  inherit (securityRootRegistry) securityRoots securityRootPaths;

  inherit darwinPackageAliases;

  # Flat keying: one derivation per `checks.<system>.<name>`, as the flake schema
  # and `nix flake check` require. The `.#check` gate and blast-radius consume
  # the sharded `ciChecks` instead, so this output is not what CI enumerates.
  # `forkChecks` is merged on EVERY system (not just x86_64-linux like the
  # rest of `catalogFor`): the patched sources are cheap, platform-relevant
  # derivations, so `nix build .#checks.aarch64-darwin.patched-src-clippy`
  # validates the series against a local Darwin build right after a flake update.
  checks = catalogFor rustPackageTestSets.flat // forkChecks;
  # Closure build gates, keyed `<fork>.<patch>` (see the binding above). A
  # non-schema output like `ciChecks`, exposed per system so a darwin host can
  # gate-build natively before an upstream PR.
  # Sharded keying for the memory-bounded CI evaluator (nix-fast-build /
  # nix-eval-jobs / blast-radius): each package's per-#[test] checks sit under one
  # `recurseForDerivations` group, so the evaluator lists cheap per-package names
  # at the root and forces each crate's manifest IFD in its own worker job
  # (ENG-2201). Not a `checks.<system>.<name>` output, because a non-derivation
  # there fails the flake schema. The patched-src checks are plain derivations,
  # so they key identically in both views.
  ciChecks = catalogFor rustPackageTestSets.sharded // forkChecks;

  # `nix fmt` runs alejandra directly on the paths it is given. A single `-q`
  # (`--quiet`) drops alejandra's informational chatter -- the
  # "Congratulations! Your code complies with the Alejandra style." success
  # line and the rotating "Special thanks ... for being a sponsor of
  # Alejandra" promo -- while still surfacing genuine formatting/parse errors
  # (a second `-q` would suppress those too, which we do not want). The
  # lint-fix `nix` lane above already runs `alejandra --quiet`; this wraps the
  # interactive `nix fmt` entrypoint the same way. A makeWrapper wrapper (not a
  # hand-rolled shell script, per no-write-shell-application) over the cached
  # `pkgs.alejandra` store path, so there is nothing extra to rebuild.
  formatter = pkgs.symlinkJoin {
    name = "alejandra-quiet";
    paths = [pkgs.alejandra];
    nativeBuildInputs = [pkgs.makeWrapper];
    postBuild = ''
      # shell
      wrapProgram $out/bin/alejandra --add-flags --quiet
    '';
    meta.mainProgram = "alejandra";
  };

  # Per-TU content-addressed kernel build (kbuild-unit, #3411), exposed under
  # `legacyPackages` so `nix build .#kernel-unit.vmlinux` resolves while the
  # two-stage IFD plan (a full monolithic kbuild at eval time) stays out of
  # `packages` and every gate closure that enumerates it (flake-check,
  # blast-radius, cache-push). x86_64-linux only: the plan replays gcc/binutils
  # saved commands, so there is no Darwin or cross lane to offer.
  legacyPackages = lib.optionalAttrs (system == "x86_64-linux") {
    kernel-unit = (ix.kernelUnitFor pkgs).buildKernel {
      inherit (pkgs.linux_6_12) src;
    };
    # defconfig-scale lane (#3412): thousands of units per eval, kept here
    # solely so the eval-cost measurement has a stable attr to time.
    kernel-unit-defconfig = (ix.kernelUnitFor pkgs).buildKernel {
      inherit (pkgs.linux_6_12) src;
      configTarget = "defconfig";
    };
    # Static fallback lane (#3413): keeps the ccache plan strategy exercised
    # so a config whose plan-time tooling rejects skeleton stub objects has a
    # validated escape hatch to flip to (strategy selection is per-lane and
    # never auto-detected; Nix cannot catch a failed drv, and a silent flip
    # would hide skeleton regressions).
    kernel-unit-ccache = (ix.kernelUnitFor pkgs).buildKernel {
      inherit (pkgs.linux_6_12) src;
      planStrategy = "ccache";
    };
  };

  # `nix run .#bench` runs the repo's self-demo perf job (timing + RSS + custom
  # metrics, gated on regressions). The flake's package-with-mainProgram
  # convention already gives `nix run .#indexbench` for the bare CLI; this `apps`
  # entry is the named perf-job entry point the framework documents.
  apps = {
    bench = {
      type = "app";
      program = lib.getExe indexbenchSelfDemo.app;
      meta.description = "Run the indexbench self-demo perf suite";
    };
  };

  # `nix develop .#bench` drops into a shell with the bench + profiling tools.
  # tango is already a workspace dependency (built per-crate by cargo-unit); the
  # shell adds the out-of-process profilers a bench author reaches for.
  devShells = {
    default = pkgs.mkShellNoCC {
      packages = [
        repoPackages.astlog
        pkgs.alejandra
      ];
    };

    bench = pkgs.mkShellNoCC {
      packages = [
        indexbench
        pkgs.hyperfine
        pkgs.valgrind
        pkgs.samply
        pkgs.jemalloc
      ];
    };
  };
}
