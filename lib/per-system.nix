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
    meta.description = "One lint stage (alejandra | statix | deadnix | astlog | astlog-rust | astlog-elixir | filenames | dirnames | ruff | clone); driven by `lint`";
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
          '(^|/)(package-lock|lock)\.json$'
          '(^|/)(pins|manifest)\.json$'
          '^\.(claude|vscode|zed)/settings\.json$'
          '^\.vscode/extensions\.json$'
          '^\.github/user-owners\.json$'
          '(^|/)(dag|upstream-status)\.json$'
          '(^|/)(fixtures?[^/]*|snapshots?|catalogs?|metadata|sounds|seeds)/.*\.json$'
          '^examples/.*\.json$'
          '^packages/agent/claude-code/system-prompts/models\.json$'
          '^packages/agent/system-prompt-eval-viewer/src/sample\.json$'
          '^packages/code/code-highlight/src/islands-theme\.json$'
          # Generated by `tree-sitter generate` and embedded by the grammar
          # crate's lib.rs (see packages/code/tree-sitter-nix/README.md).
          '^packages/code/tree-sitter-nix/src/node-types\.json$'
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
        error make { msg: "specify a stage: alejandra | statix | deadnix | astlog | astlog-rust | astlog-elixir | filenames | dirnames | ruff | clone" }
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
    "filenames"
    "dirnames"
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
            print "lint --fix: applied; the verdict-only stages (astlog, filenames, dirnames, clone) and unfixable findings still need `nix run .#lint`"
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

  # De-forked patched sources + patch-DAG invariants exposed as
  # `checks.<system>.{patched-src,patch-dag}-<name>`. `patched-src-<name>` is the
  # seconds-fast "does the series still apply" gate, built from the same
  # `patchedSrc` the packages consume against the same raw upstream inputs, so it
  # can never drift from the real build. `patch-dag-<name>` is its textual
  # sibling, validating the committed `dag.json` against the pinned base. Both are
  # built by the shared `ix.mkForkChecks` (lib/mk-fork-checks.nix) — the one owner
  # of these check derivations, reused verbatim by ix for its own forks — driven
  # by index's fork-package list so a new entry there joins this set with no
  # change here. Each raw input is exposed on the `ix` handle as `<name>Src` (see
  # lib/default.nix sharedHelpers). Merged on every system, so
  # `nix build .#checks.aarch64-darwin.patch-dag-clippy` validates natively.
  forkChecks = ix.mkForkChecks {
    inherit pkgs;
    patchedSrcFor = ix.patchedSrcFor pkgs;
    inherit (ix) forkPackages;
    dagCheckSrc = ix.forkDagCheckSrc;
    forkSrcInputs = lib.genAttrs (map (fork: fork.name) ix.forkPackages) (
      name: ix."${name}Src"
    );
    patchesRoot = paths.root;
    flakeLock = lib.importJSON (paths.root + "/flake.lock");
  };

  # Per-attempt-patch closure build gates (RFC 0010 A3, #2098): for each fork
  # opted in via `closureGates = true` in lib/fork-packages.nix, the fork
  # package rebuilt with its series restricted to each attempt-marked patch's
  # dag.json closure -- exactly the standalone series `upstream-pr` ships
  # upstream, so a red gate means the upstream PR would be broken. The gate
  # derivations live on the opted-in package's `passthru.closureGates` (the
  # package owns its own re-instantiation; see packages/nix/nix/default.nix);
  # this map only keys them by fork name so the scheduled fork-closure-gates
  # workflow and the `upstream-sync --open` preflight can `nix eval` the set
  # and `nix build .#forkClosureGates.<system>.<fork>."<patch>"`. NEVER merged
  # into `checks`/`ciChecks`: these are heavy full-package builds, and per-PR
  # flake-check cost must stay flat (the attrset is lazy, so enumerating it
  # forces nothing heavy).
  forkClosureGates = let
    # Fork name -> the repo package carrying that fork's gates. A fork flagged
    # `closureGates = true` without an entry here fails eval loudly instead of
    # silently publishing no gates.
    gatePackages = {
      nix = repoPackages.nix-ix;
    };
  in
    lib.genAttrs' (lib.filter (fork: fork.closureGates or false) ix.forkPackages) (
      fork:
        lib.nameValuePair fork.name
        (gatePackages.${fork.name}
          or (throw "lib/per-system.nix: fork `${fork.name}` sets closureGates = true in lib/fork-packages.nix but gatePackages maps no package for it"))
        .closureGates
    );

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
        # These three ARE the whole eval-time closure: importing the generated
        # catalog forces it, and it references only `unitGraphJson` and `vendorDir`
        # (the cargo-lock it also reads is a plain flake source path, always
        # present). `cargo-vendor-config.toml` is not a fourth root: it is a
        # build input of the `unitGraphJson` builder, not on the import path and
        # not in `vendorDir`'s closure, so substituting `unitGraphJson`'s output
        # makes it moot -- the Mac never runs that builder.
        {
          "cross-ifd-${target}-units-nix" = units.generatedUnitCatalog;
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
    # shared unit graph (same path ix-vt's default.nix uses). The library unit
    # key is the Cargo package name with dashes underscored.
    packageTestsFor = entry:
      if entry.packageSet != null
      then
        (
          lib.attrByPath entry.packageSet.attrPath
          (throw "packages/${entry.relativePath}/package.nix: passthruTests needs packageSet.attrPath")
          repoPackages
        ).passthru.tests or {
        }
      else
        (cargoUnit.selectLibraryWithTests rustWorkspace.units {
          library = lib.replaceStrings ["-"] ["_"] entry.id;
          packageName = entry.id;
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

  lintSource = fs.toSource {
    inherit (paths) root;
    fileset = fs.gitTracked paths.root;
  };

  # Just the astlog rules file plus its fixture pairs, so the rules self-test
  # below only rebuilds when the rules or fixtures change, not on every
  # tracked-file edit the way `lintSource` does.
  astlogRulesSource = fs.toSource {
    inherit (paths) root;
    fileset = fs.intersection (fs.gitTracked paths.root) (paths.root + "/astlog-rules");
  };

  andrewZellij = import (paths.root + "/users/andrewgazelka/config/zellij") {
    configRoot = paths.root + "/users/andrewgazelka/config";
    inherit (pkgs) lib stdenvNoCC zellijPlugins;
    xdgConfigHome = "/Users/andrewgazelka/.config";
  };
  andrewZellijConfig = pkgs.writeText "andrewgazelka-zellij.kdl" (ix.kdl.render andrewZellij.settings);
  andrewNushellConfig = paths.root + "/users/andrewgazelka/config/nushell";

  tests = import paths.tests {
    inherit
      nixpkgs
      ix
      paths
      home-manager
      ;
  };
  # This is an exec-style toolchain wrapper, so the checked Bash writer keeps
  # Python's exit and signal behavior instead of interposing another process.
  bumpRustNightly = ix.writeBashApplication pkgs {
    name = "bump-rust-nightly";
    runtimeInputs = [pkgs.python3];
    text = ''
      export SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt
      exec python3 \
        ${paths.root + "/.github/scripts/bump-rust-nightly.py"} "$@"
    '';
  };
  # The source catalog is canonical Linux data, but the command that installs
  # it into a checkout must be executable on every supported developer host.
  # Atomic replacement and symlink refusal are easier to audit as checked
  # Bash than as a cross-platform sequence of Nushell filesystem operations.
  updateCargoUnitCatalog = ix.writeBashApplication pkgs {
    name = "update-cargo-unit-catalog";
    runtimeInputs = [
      pkgs.coreutils
      pkgs.gitMinimal
    ];
    text = ''
      repo_root=$(git rev-parse --show-toplevel)
      destination="$repo_root/tests/fixtures/cargo-unit-hello/unit-catalog"
      for directory in \
        "$repo_root/tests" \
        "$repo_root/tests/fixtures" \
        "$repo_root/tests/fixtures/cargo-unit-hello"; do
        if [[ ! -d "$directory" || -L "$directory" ]]; then
          printf 'refusing unsafe catalog directory: %s\n' "$directory" >&2
          exit 1
        fi
      done
      if [[ ! -f "$destination" || -L "$destination" ]]; then
        printf 'refusing unsafe catalog destination: %s\n' "$destination" >&2
        exit 1
      fi
      temporary=$(mktemp "$destination.tmp.XXXXXX")
      trap 'rm -f "$temporary"' EXIT
      install -m 0644 ${tests.cargoUnitGeneratedCatalog} "$temporary"
      mv -fT "$temporary" "$destination"
      trap - EXIT
      printf 'updated %s\n' "$destination"
    '';
  };

  updateRustNightlyWorkflowCheck =
    pkgs.runCommandLocal "update-rust-nightly-workflow-check" {
      nativeBuildInputs = [
        pkgs.bash
        pkgs.jq
        pkgs.yq-go
      ];
    } ''
      bash ${paths.root + "/.github/scripts/update-rust-nightly-test.sh"} \
        ${paths.root + "/.github/workflows/update-rust-nightly.yml"}
      touch "$out"
    '';

  exampleFleets = ix.exampleFleetsFor {hostSystem = system;};

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
  # `examples/oci` with the same hierarchical shape as fleet examples, but
  # return images instead of fleet plans and are exposed as opt-in packages only.
  nonNixExampleImages =
    lib.mapAttrs'
    (
      name: entry:
        lib.nameValuePair "non-nix-${name}" (
          import (entry.path + "/ix.nix") {
            index = {
              lib = ix;
            };
          }
        )
    )
    (
      ix.discoverTree {
        root = paths.examples + "/oci";
        requiredFiles = ["ix.nix"];
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
            cargo-unit-catalog = tests.cargoUnitCatalog;
            cargo-unit-real-workspaces = tests.cargoUnitRealWorkspaces;
            cargo-unit-prebuilt-library = tests.cargoUnitPrebuiltLibrary;
            sdk-rust-prebuilt = tests.sdkRustPrebuilt;
            # Strict zuban + ruff ANN gate over the public ix-sdk Python sources
            # (ENG-3131); the SDK is setuptools-built, so this is its build-time
            # enforcement in place of a buildUvApplication pyChecker flag.
            sdk-python-strict = tests.sdkPythonStrict;
          }
          // rustPackageSet;
        explicitChecks = {
          inherit (tests) eval;
          update-rust-nightly-workflow = updateRustNightlyWorkflowCheck;
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
          # Skills and subagents are rendered live by the SessionStart hook.
          # This gate forces both materialized directories to build.
          agent-skills = pkgs.runCommand "agent-skills-check" {} ''
            test -d ${skillsDir}
            test -d ${agentsDir}
            mkdir -p "$out"
          '';
          # Pins the last-applied 3-way merge behind homeModules.mutable-json:
          # first-install, preserve an app-written key, enforce a key the app
          # changed, prune a key Nix stopped declaring, and keep a sibling key
          # while a declared array is replaced atomically.
          mutable-json-merge =
            pkgs.runCommand "mutable-json-merge-check" {nativeBuildInputs = [pkgs.jq];}
            ''
              prog=${ix.mutableJson.mergeProgram}
              run() { jq -ncS --argjson last "$1" --argjson live "$2" --argjson new "$3" -f "$prog"; }
              check() {
                expected=$(printf '%s' "$2" | jq -cS .)
                if [ "$expected" != "$3" ]; then
                  echo "FAIL $1: expected $expected got $3" >&2
                  exit 1
                fi
                echo "ok $1"
              }
              check first-install '{"permissions":{"defaultMode":"bypass"}}' \
                "$(run '{}' '{}' '{"permissions":{"defaultMode":"bypass"}}')"
              check preserve-app-key '{"permissions":{"defaultMode":"bypass"},"theme":"dark"}' \
                "$(run '{"permissions":{"defaultMode":"bypass"}}' '{"permissions":{"defaultMode":"bypass"},"theme":"dark"}' '{"permissions":{"defaultMode":"bypass"}}')"
              check enforce-changed '{"permissions":{"defaultMode":"bypass"},"theme":"dark"}' \
                "$(run '{"permissions":{"defaultMode":"bypass"}}' '{"permissions":{"defaultMode":"off"},"theme":"dark"}' '{"permissions":{"defaultMode":"bypass"}}')"
              check prune-dropped '{"a":1,"c":3}' \
                "$(run '{"a":1,"b":2}' '{"a":1,"b":2,"c":3}' '{"a":1}')"
              check nested-atomic-array '{"p":{"allow":["x"]},"t":1}' \
                "$(run '{"p":{"allow":["x"]}}' '{"p":{"allow":["x","y"]},"t":1}' '{"p":{"allow":["x"]}}')"
              # Divergent live shape at a path we stop declaring must not abort:
              # the app replaced object `permissions` with a scalar, Nix dropped it.
              check divergent-live-shape '{"permissions":"all"}' \
                "$(run '{"permissions":{"defaultMode":"x"}}' '{"permissions":"all"}' '{}')"
              mkdir -p "$out"
            '';
          # Pins the formatProvenance seam (lib/util/format-provenance.nix): a
          # wrapped comment-capable generator must emit the `# generated by
          # <file>:<line>` header as line 1 and keep the format's own rendering
          # intact below it, for both an unsafeGetAttrPos-shaped position and
          # an explicit { file, line } pair.
          format-provenance = let
            sample = {greeting = "hello";};
            tomlFile =
              (ix.formatProvenance.withHeader pkgs {
                format = pkgs.formats.toml {};
                position = builtins.unsafeGetAttrPos "greeting" sample;
              }).generate "sample.toml"
              sample;
            kvFile = (ix.formatProvenance.withHeader pkgs {
              format = pkgs.formats.keyValue {};
              position = {
                file = "lib/per-system.nix";
                line = 1;
              };
            }).generate "sample.env" {GREETING = "hello";};
          in
            pkgs.runCommand "format-provenance-check" {} ''
              head -n1 ${tomlFile} | grep -Eqx '# generated by .+/per-system\.nix:[0-9]+'
              grep -qx 'greeting = "hello"' ${tomlFile}
              head -n1 ${kvFile} | grep -qx '# generated by lib/per-system\.nix:1'
              grep -qx 'GREETING=hello' ${kvFile}
              mkdir -p "$out"
            '';
          # Offline schema gate for the loader manifests. `deepSeq` forces
          # every Paper / Velocity / Fabric per-version lock through
          # `readLoaderManifest` in `lib/artifacts.nix`, so malformed JSON or a
          # missing key fires here before any image starts evaluating. The
          # forced surface is the parsed-and-validated manifest data, not the
          # wrapped `fetchurl` derivations, to keep this check pure eval.
          loader-manifests = let
            forced = builtins.deepSeq ix.artifacts.minecraft.loaderManifests "ok";
          in
            pkgs.runCommand "loader-manifests-check" {} ''
              printf '%s\n' '${forced}' > "$out"
            '';
          # Rule self-test for the astlog lint rules (nix.astlog + rust.astlog):
          # every (lint ...) declaration must have a committed fixture pair and
          # fire exactly on the violating one, driven through the same `astlog
          # scan --json` surface the lint gate uses. A lint that never fires in
          # tests is unproven (its query may have silently stopped matching), so
          # a missing or non-firing fixture fails the build, as does a rule
          # without a lint declaration (it would silently drop out of the gate).
          # Fixtures are stored as `.fixture` (not `.nix`/`.rs`) so the repo lint
          # stages (alejandra / statix / deadnix / astlog itself) never scan the
          # deliberately-violating snippets; the check stages each back to its
          # ruleset's extension (`.nix` for nix.astlog, `.rs` for rust.astlog —
          # astlog selects the grammar by file extension) before running the
          # binary. `scan` exits nonzero on the violating fixture by design, so
          # the jq pipelines deliberately take the JSON regardless of exit code.
          astlog-rules =
            pkgs.runCommand "astlog-rules-check"
            {
              nativeBuildInputs = [
                repoPackages.astlog
                pkgs.jq
              ];
            }
            ''
              root=${astlogRulesSource}/astlog-rules
              tests="$root/tests"
              fail=0
              # Each ruleset paired with the source extension its fixtures take.
              check_ruleset() {
                rules="$1"
                ext="$2"
                # Rules without a (lint ...) are legitimate helper relations
                # (joins/negation need intermediate relations), so they are not
                # required to back a lint. The meaningful checks remain: every
                # lint has a good/bad fixture pair that fires/stays-clean, and
                # every fixture dir backs some lint. `astlog` itself rejects a
                # lint that names an undefined relation at parse time.
                for rule in $(sed -n 's/^(lint \([a-z0-9-]*\).*/\1/p' "$rules" | sort -u); do
                  dir="$tests/$rule"
                  if [ ! -f "$dir/bad.fixture" ] || [ ! -f "$dir/good.fixture" ]; then
                    echo "lint $rule has no fixture pair under astlog-rules/tests/$rule" >&2
                    fail=1
                    continue
                  fi
                  work=$(mktemp -d)
                  cp "$dir/bad.fixture" "$work/bad.$ext"
                  cp "$dir/good.fixture" "$work/good.$ext"
                  # `astlog scan` exits nonzero on a violating fixture by
                  # design; capture its JSON (`|| true` so the by-design exit
                  # does not abort the `set -o pipefail` build) and count
                  # separately, rather than piping straight into jq.
                  bad_json=$(astlog scan "$rules" "$work/bad.$ext" --json || true)
                  good_json=$(astlog scan "$rules" "$work/good.$ext" --json || true)
                  bad=$(jq --arg r "$rule" '[.[] | select(.rule == $r)] | length' <<<"$bad_json")
                  good=$(jq --arg r "$rule" '[.[] | select(.rule == $r)] | length' <<<"$good_json")
                  if [ "$bad" = 0 ]; then
                    echo "lint $rule did not fire on its violating fixture" >&2
                    fail=1
                  fi
                  if [ "$good" != 0 ]; then
                    echo "lint $rule fired $good finding(s) on its valid fixture" >&2
                    fail=1
                  fi
                done
              }
              check_ruleset "$root/nix.astlog" nix
              check_ruleset "$root/rust.astlog" rs
              check_ruleset "$root/cargo.astlog" toml
              check_ruleset "$root/elixir.astlog" ex
              # Every fixture dir must back a lint in one of the rulesets.
              for dir in "$tests"/*/; do
                rule=$(basename "$dir")
                if ! grep -q "^(lint $rule " "$root/nix.astlog" "$root/rust.astlog" "$root/cargo.astlog" "$root/elixir.astlog"; then
                  echo "fixture dir astlog-rules/tests/$rule matches no lint" >&2
                  fail=1
                fi
              done
              if [ "$fail" != 0 ]; then
                exit 1
              fi
              mkdir -p "$out"
            '';
          # End-to-end proof that scipql resolves SCIP monikers and acts only on
          # the right symbol, exercising all three surfaces (query / fix /
          # rename) of the real pipeline. The wrapped CLI bakes rust-analyzer +
          # the pinned toolchain + souffle; the fixture is a dependency-free
          # crate with a `net::Socket` and a same-named `mock::Socket`, so
          # rust-analyzer's `cargo metadata` needs no network. Tree-sitter
          # (astlog) could not tell the two `Socket`s apart; this is the
          # semantic-disambiguation guarantee.
          scipql-e2e =
            pkgs.runCommand "scipql-e2e-check"
            {
              nativeBuildInputs = [repoPackages.scipql];
            }
            ''
              export HOME="$TMPDIR/home"
              mkdir -p "$HOME"
              cp -r ${
                builtins.path {
                  name = "scipql-two-sockets-fixture";
                  path = paths.packagesRoot + "/code/scipql/tests/fixtures/two-sockets";
                }
              } work
              chmod -R u+w work
              cd work
              fail=0

              scipql index . -o index.scip

              # query: the two same-named structs resolve to distinct monikers.
              # (printf, not a heredoc: a heredoc terminator would not sit at
              # column 0 after Nix strips the indented string's indentation.)
              printf '%s\n' \
                '.decl sockets(sym:symbol)' \
                '.output sockets' \
                'sockets(s) :- occurrence(s, _, _, _, "definition"), symbol_info(s, _, "Socket").' \
                > sockets.dl
              q=$(scipql query index.scip sockets.dl)
              echo "$q" | grep -q 'net/Socket#' || { echo "query: missing net/Socket# definition" >&2; fail=1; }
              echo "$q" | grep -q 'mock/Socket#' || { echo "query: missing mock/Socket# definition" >&2; fail=1; }

              # fix: the replacement text is COMPUTED in datalog (cat + a join to
              # the display name), not a constant, and still scoped to net by moniker.
              printf '%s\n' \
                'edit(path, start, end, cat("Net", name)) :-' \
                '  occurrence(sym, path, start, end, _),' \
                '  symbol_info(sym, _, name),' \
                '  substr(sym, strlen(sym) - strlen("net/Socket#"), strlen("net/Socket#")) = "net/Socket#".' \
                > netname.dl
              d=$(scipql fix index.scip netname.dl)
              echo "$d" | grep -q 'NetSocket' || { echo "fix: datalog-computed replacement (cat) did not apply" >&2; fail=1; }
              echo "$d" | grep -q 'src/mock.rs' && { echo "fix: computed edit wrongly touched mock.rs" >&2; fail=1; }

              # rename: apply to disk, then assert the net struct + its reference
              # changed while mock::Socket and the net struct's own fd field did not.
              scipql rename index.scip 'net/Socket#' Stream --write
              grep -q 'pub struct Stream' src/net.rs || { echo "rename: net::Socket was not renamed" >&2; fail=1; }
              grep -q 'net::Stream' src/lib.rs || { echo "rename: the net::Socket reference was not renamed" >&2; fail=1; }
              grep -q 'pub struct Socket' src/mock.rs || { echo "rename: mock::Socket was wrongly changed" >&2; fail=1; }
              grep -q 'pub fd: i32' src/net.rs || { echo "rename: the struct's own fd field was wrongly renamed" >&2; fail=1; }

              if [ "$fail" != 0 ]; then
                echo "--- net.rs ---" >&2; cat src/net.rs >&2
                echo "--- mock.rs ---" >&2; cat src/mock.rs >&2
                echo "--- lib.rs ---" >&2; cat src/lib.rs >&2
                exit 1
              fi
              mkdir -p "$out"
            '';
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
          lint = pkgs.runCommand "ix-lint" {nativeBuildInputs = [pkgs.coreutils];} ''
            cp -R ${lintSource} source
            chmod -R u+w source
            cd source
            ${lib.getExe lint}
            mkdir -p "$out"
          '';
          # Acceptance for the fixer lanes (#3432), both halves in one gate: a
          # deliberately violating tree becomes clean under the same check
          # stages after one pass through its lane, and fixing the already-
          # fixed tree is byte-identical (the no-op property that makes a
          # clean tree a CA cache hit all the way down and `--fix` safe to
          # re-run). Fixtures are built inline rather than committed:
          # committed files with these violations would fail the repo's own
          # lint stages (the same reason the astlog fixtures live as
          # `.fixture`). Each fixture is first asserted to FAIL its check
          # stage, so tool or selector drift that stops exercising a fixer
          # turns this check red instead of silently proving nothing.
          lint-fix = let
            # One fixable finding per nix-lane tool: an unused binding
            # (deadnix --edit), useless parens (statix fix), misformatting
            # (alejandra). The unused lambda pattern also pins the lane's
            # ordering: deadnix deletes `unusedArg` leaving `{}:`, a fresh
            # empty_pattern finding only a statix fix run AFTER deadnix
            # repairs, so a statix-first lane fails this check's post-fix
            # statix stage.
            violatingNix = pkgs.writeTextDir "fixture.nix" ''
              {unusedArg}: let
                unused = 1;
                greeting = ("hello");
              in {   inherit greeting; }
            '';
            # UP024 (IOError -> OSError): inside the shared selector's UP
            # family with a SAFE autofix, so the lane's plain `--fix` applies
            # it. Not C408 and friends: their fixes are unsafe-gated, the lane
            # deliberately never passes `--unsafe-fixes`, and an unsafe-only
            # fixture would survive the lane and fail this check's post-fix
            # gate. Module-level so the ANN rules are satisfied without
            # annotations.
            violatingPython = pkgs.writeTextDir "fixture.py" ''
              raise IOError("fixture")
            '';
            # A one-crate workspace whose main.rs cargo fmt rewrites: the
            # manifest is what cargo-fmt's `cargo metadata --no-deps` walks,
            # so this also proves the lane works from manifests alone (no
            # Cargo.lock, no dependency sources, no network).
            violatingRust = pkgs.runCommand "rust-fixture" {} ''
              mkdir -p "$out/src"
              cat > "$out/Cargo.toml" <<'EOF'
              [package]
              name = "fixture"
              version = "0.0.0"
              edition = "2021"
              EOF
              printf 'fn main(){println!("fixture") ;}\n' > "$out/src/main.rs"
            '';
            fixedNix = lintFix.lanes.nix violatingNix;
            fixedPython = lintFix.lanes.python violatingPython;
            fixedRust = lintFix.lanes.rust violatingRust;
            fixedTree = lintFix.unite "lint-fix-fixture-fixed" [
              fixedNix
              fixedPython
              fixedRust
            ];
          in
            pkgs.runCommand "lint-fix-check"
            {
              nativeBuildInputs = [
                pkgs.alejandra
                pkgs.deadnix
                pkgs.ruff
                pkgs.statix
                lintFix.rustFmtToolchain
              ];
            }
            ''
              # Pre-fix: every tool must find its planted violation.
              if alejandra --check ${violatingNix}/fixture.nix; then
                echo "fixture stopped violating alejandra" >&2
                exit 1
              fi
              if statix check ${violatingNix}; then
                echo "fixture stopped violating statix" >&2
                exit 1
              fi
              if deadnix --fail ${violatingNix}; then
                echo "fixture stopped violating deadnix" >&2
                exit 1
              fi
              if ruff check ${ix.ruffAnnArgs} --no-cache ${violatingPython}/fixture.py; then
                echo "fixture stopped violating ruff" >&2
                exit 1
              fi
              # cargo wants a writable home even for `fmt --check`.
              export CARGO_HOME="$TMPDIR/cargo-home"
              if (cd ${violatingRust} && cargo fmt --all --check); then
                echo "fixture stopped violating cargo fmt" >&2
                exit 1
              fi

              # Post-fix: the united tree passes the same check stages the
              # lint gate runs, through the same stage binary.
              cp -R ${fixedTree} fixed
              chmod -R u+w fixed
              cd fixed
              ${lib.getExe lintStage} alejandra
              ${lib.getExe lintStage} statix
              ${lib.getExe lintStage} deadnix
              ${lib.getExe lintStage} ruff
              # No rustfmt stage exists in the lint gate (#3433 ships the
              # fixer only), so the post-fix rust gate is cargo fmt itself,
              # from the same pinned toolchain the lane ran.
              cargo fmt --all --check
              cd ..

              # No-op: a second pass over each already-fixed lane output must
              # be byte-identical.
              diff -r ${fixedNix} ${lintFix.lanes.nix fixedNix}
              diff -r ${fixedPython} ${lintFix.lanes.python fixedPython}
              diff -r ${fixedRust} ${lintFix.lanes.rust fixedRust}
              mkdir -p "$out"
            '';
          filename-policy =
            pkgs.runCommand "filename-policy-check"
            {
              nativeBuildInputs = [pkgs.coreutils];
            }
            ''
              mkdir source
              cd source
              touch repository-config.json zellij-layout.kdl
              if ${lib.getExe lintStage} filenames >output 2>&1; then
                echo "filename policy accepted repository-config.json" >&2
                exit 1
              fi
              grep -F "repository-config.json" output
              grep -F "zellij-layout.kdl" output
              touch "$out"
            '';
          # Both halves of the dirnames stage: a marker-less doubled segment is
          # flagged, an eponym package root (package.nix) is exempt.
          dirname-policy =
            pkgs.runCommand "dirname-policy-check"
            {
              nativeBuildInputs = [pkgs.coreutils];
            }
            ''
              mkdir source
              cd source
              mkdir -p packages/foo/foo packages/bar/bar
              touch packages/bar/bar/package.nix
              if ${lib.getExe lintStage} dirnames >output 2>&1; then
                echo "dirname policy accepted packages/foo/foo" >&2
                exit 1
              fi
              grep -F "packages/foo/foo" output
              if grep -F "packages/bar/bar" output; then
                echo "dirname policy exempted nothing: flagged the eponym package packages/bar/bar" >&2
                exit 1
              fi
              touch "$out"
            '';
          zellij-config = pkgs.runCommand "zellij-config-check" {nativeBuildInputs = [pkgs.zellij];} ''
            export HOME="$TMPDIR/home"
            mkdir -p "$HOME" "$out"
            zellij --config ${andrewZellijConfig} setup --check >"$out/check.txt"
          '';
          nushell-config =
            pkgs.runCommand "nushell-config-check"
            {
              nativeBuildInputs = [
                pkgs.binutils
                pkgs.jq
                # The fork package, not pkgs.nushell: the deployed shell that
                # executes this config is newer than the repo's nixpkgs pin
                # (0.114 names vs 0.113.1), so the check must run a
                # repo-controlled interpreter that tracks upstream (#3428).
                repoPackages.nushell
              ];
            }
            ''
              export HOME="$TMPDIR/home"
              config_dir=$(nu --no-config-file -c '$nu.default-config-dir')
              mkdir -p "$(dirname "$config_dir")"
              cp -R ${andrewNushellConfig} "$config_dir"
              cd "$config_dir"
              diagnostics="$TMPDIR/diagnostics.jsonl"
              nu --no-config-file --ide-check 100 config.nu > "$diagnostics"
              if ! jq -s -e 'map(select(.type == "diagnostic")) | length == 0' "$diagnostics" >/dev/null; then
                jq -s 'map(select(.type == "diagnostic"))' "$diagnostics" >&2
                exit 1
              fi
              if [[ $(nu --no-config-file -c 'nu-check functions/infra/status.nu') != true ]]; then
                echo "infra status probe did not parse" >&2
                exit 1
              fi
              for test in tests/test_*.nu; do
                nu --no-config-file "$test"
              done
              touch "$out"
            '';
          # Exercises the trusted half of the blast-radius PR comment: the
          # validate/render jq embedded in its workflow, extracted from the YAML so
          # the test can't drift from what the trusted comment job runs. The
          # report-building logic lives in the `blast-radius` Rust crate and is
          # covered by its own unit tests. See packages/blast-radius/tests/blast-radius-test.sh.
          blast-radius-test =
            pkgs.runCommand "blast-radius-test"
            {
              nativeBuildInputs = [
                pkgs.bash
                pkgs.coreutils
                pkgs.diffutils
                pkgs.jq
                pkgs.yq-go
              ];
            }
            ''
              cp -R ${lintSource} source
              chmod -R u+w source
              cd source
              export HOME="$TMPDIR/home"
              mkdir -p "$HOME"
              bash packages/blast-radius/tests/blast-radius-test.sh
              mkdir -p "$out"
            '';
          # Proves the Linux→macOS cross toolchain actually emits a Darwin object,
          # which a successful build alone does not assert. `file` reads the Mach-O
          # header; a regression in the zig/SDK wiring fails here on x86_64-linux CI
          # rather than silently shipping a wrong-arch binary.
          cross-darwin-smoke = pkgs.runCommand "cross-darwin-smoke" {nativeBuildInputs = [pkgs.file];} ''
            bin=${crossPackages.dag-runner-aarch64-apple-darwin}/bin/dag-runner
            info=$(file -b "$bin")
            echo "$info"
            case "$info" in
              *Mach-O*arm64*) ;;
              *)
                echo "expected Mach-O arm64, got: $info" >&2
                exit 1
                ;;
            esac
            mkdir -p "$out"
          '';
          cross-darwin-web-monitor-smoke =
            pkgs.runCommand "cross-darwin-web-monitor-smoke" {nativeBuildInputs = [pkgs.file];}
            ''
              pkg=${crossPackages.nix-web-monitor-aarch64-apple-darwin}
              bin=$pkg/bin/.nix-web-monitor-unwrapped
              info=$(file -b "$bin")
              echo "$info"
              case "$info" in
                *Mach-O*arm64*) ;;
                *)
                  echo "expected Mach-O arm64, got: $info" >&2
                  exit 1
                  ;;
              esac
              read -r shebang < "$pkg/bin/nix-web-monitor"
              case "$shebang" in
                "#!/bin/sh") ;;
                *)
                  echo "expected /bin/sh wrapper, got: $shebang" >&2
                  exit 1
                  ;;
              esac
              test -f "$pkg/share/nix-web-monitor/index.html"
              mkdir -p "$out"
            '';
          site-test = siteTests.all;
        };
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
      bump-rust-nightly = bumpRustNightly;
      update-cargo-unit-catalog = updateCargoUnitCatalog;
      site-dev = site.passthru.devServer;
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
        fleetName: fleet:
          lib.mapAttrs' (
            node: path: let
              name = "example-${fleetName}-${node}";
            in
              lib.nameValuePair name {
                inherit path;
                root = mkRoot {
                  attr = "exampleFleets.${system}.${fleetName}.systemPackages.${node}";
                  inherit name owner;
                  class = "deployed-service";
                  environment = "development";
                  exposure = "internal";
                  criticality = "medium";
                  slaHours = 72;
                };
              }
          )
          fleet.systemPackages
      )
      exampleFleets;
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
  # cache useful to `ix up` while cutting the monolithic `*-oci.tar` archives that
  # dominate the run -- each is one uncompressed blob that never dedups, cold
  # every run since check.yml only eval-validates packages:
  #
  #   1. Every NixOS image is replaced by its `toplevel` closure -- the artifact
  #      `ix up` substitutes (consumers reconstruct the archive on demand via
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
    # `fleet.systemPackages` keys each node's toplevel as `<node>-system`; the
    # fleet-name prefix keeps nodes sharing a name across fleets distinct.
    exampleNodeToplevels =
      lib.concatMapAttrs (
        fleetName: fleet:
          lib.mapAttrs' (
            node: toplevel: lib.nameValuePair "${fleetName}-${node}" toplevel
          )
          fleet.systemPackages
      )
      exampleFleets;
    # Native analog of `crossIfdRoots` (adjustment 4). `crossWorkspace` with no
    # target override IS the host workspace, so these are exactly the drvs a
    # Darwin consumer's eval of the native wrappers imports.
    nativeIfdRoots = lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
      native-ifd-units-nix = crossWorkspace.units.generatedUnitCatalog;
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
  inherit forkClosureGates;
  # Sharded keying for the memory-bounded CI evaluator (nix-fast-build /
  # nix-eval-jobs / blast-radius): each package's per-#[test] checks sit under one
  # `recurseForDerivations` group, so the evaluator lists cheap per-package names
  # at the root and forces each crate's manifest IFD in its own worker job
  # (ENG-2201). Not a `checks.<system>.<name>` output, because a non-derivation
  # there fails the flake schema. The patched-src checks are plain derivations,
  # so they key identically in both views.
  ciChecks = catalogFor rustPackageTestSets.sharded // forkChecks;

  formatter = pkgs.alejandra;

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
