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
}:
let
  inherit (nixpkgs) lib;
  pkgs = import nixpkgs {
    inherit system;
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
    meta.description = "One lint stage (nixfmt | statix | deadnix | astlog | astlog-rust | astlog-elixir | ruff); driven by `lint`";
    runtimeInputs = [
      pkgs.deadnix
      pkgs.fd
      pkgs.nixfmt
      pkgs.ruff
      pkgs.statix
      repoPackages.astlog
    ];
    text = ''
      def "main nixfmt" [] {
        let nix_files = (fd --extension nix | lines)
        nixfmt --check ...$nix_files
      }
      def "main statix" [] { statix check . }
      def "main deadnix" [] { deadnix --fail --no-lambda-pattern-names . }
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
      def main [] {
        error make { msg: "specify a stage: nixfmt | statix | deadnix | astlog | astlog-rust | astlog-elixir | ruff" }
      }
    '';
  };

  lintSpec = (pkgs.formats.json { }).generate "lint-dag.json" {
    nodes = {
      nixfmt.command = [
        (lib.getExe lintStage)
        "nixfmt"
      ];
      statix.command = [
        (lib.getExe lintStage)
        "statix"
      ];
      deadnix.command = [
        (lib.getExe lintStage)
        "deadnix"
      ];
      astlog.command = [
        (lib.getExe lintStage)
        "astlog"
      ];
      "astlog-rust".command = [
        (lib.getExe lintStage)
        "astlog-rust"
      ];
      "astlog-elixir".command = [
        (lib.getExe lintStage)
        "astlog-elixir"
      ];
      ruff.command = [
        (lib.getExe lintStage)
        "ruff"
      ];
    };
  };

  lint = ix.writeNushellApplication pkgs {
    name = "lint";
    meta.description = "Run all Nix formatting and lint checks in parallel via dag-runner";
    runtimeInputs = [ repoPackages.dag-runner ];
    text = ''
      def --wrapped main [...args] {
        exec dag-runner ...$args ${lintSpec}
      }
    '';
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
  # repo-built patches of nixpkgs' 1.5.0 / v2.34.1 (same commits the flake refs
  # used to pin): the patched nix-eval-jobs (--nix-eval-jobs) resolves floating-CA
  # outputs so they report a real cacheStatus instead of always-uncached, and the
  # patched nix-fast-build makes --skip-cached skip a `local` (warm-store) output,
  # not just a remotely-`cached` one. Without both, --skip-cached re-realizes every
  # floating-CA rust unit and image closure (~1450) on every warm run. See the
  # $fast_build and $eval_jobs comments below.
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
  # `set -o pipefail`). Uses the repo-built patched nix-eval-jobs
  # (packages/nix/nix-eval-jobs, nixpkgs' v2.34.1 + the CA cacheStatus patch),
  # matching the host Nix 2.34.x; invoked directly by store path rather than
  # `nix run`.
  check = ix.writeNushellApplication pkgs {
    name = "check";
    meta.description = "Run the full CI gate: build .#ciChecks.x86_64-linux and eval-validate .#packages.x86_64-linux";
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
      # Patched nix-eval-jobs (packages/nix/nix-eval-jobs): the stock binary
      # reports `local`/`notBuilt` for floating content-addressed outputs even
      # when they are in cache.ix.dev, so --skip-cached rebuilt every CA rust
      # unit (~1434) on every run. The patch resolves the CA output realisation
      # against the substituters so a warm unit reports `cached` and is skipped.
      # See nix#12128 / nix-eval-jobs#403. Built for x86_64-linux (the CI gate
      # system); `check` itself is x86_64-linux-only.
      const eval_jobs = "${lib.getExe repoPackages.nix-eval-jobs}"

      def main [] {
        # ca-derivations: the rust workspace units default to
        # `contentAddressed = true` (lib/rust/cargo-unit.nix), so evaluating
        # `.#ciChecks.x86_64-linux` resolves floating content-addressed drvs. The
        # evaluator (nix-eval-jobs, which nix-fast-build wraps) needs the
        # `ca-derivations` experimental feature, or it aborts with
        # "experimental Nix feature 'ca-derivations' is disabled". The flake's
        # nixConfig.extra-experimental-features carries it via
        # accept-flake-config; `--option extra-experimental-features` here pins
        # it for the build pool too so the gate is self-contained.
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
              "--flake" ".#ciChecks.x86_64-linux"
              # Drive nix-fast-build's evaluator with the patched nix-eval-jobs
              # (CA cacheStatus fix) so --skip-cached actually skips warm CA
              # units rather than rebuilding the lot.
              "--nix-eval-jobs" $eval_jobs
              "--eval-max-memory-size" "6144"
              "--eval-workers" "16"
              "--skip-cached"
              "--no-nom"
              "--no-link"
              "--result-format" "json"
              "--result-file" "check-results.json"
              "--option" "accept-flake-config" "true"
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
            let inst = $".#ciChecks.x86_64-linux.($f.attr)"
            # Fast path: replay the retained build log via `nix log` (works for
            # input-addressed checks like the browser smoke test).
            let drv = (
              ^nix eval --raw
                --option accept-flake-config true
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
            } else {
              # A content-addressed build (the rust units default to CA) keeps
              # its log under the *resolved* drv, which `nix log` cannot fetch by
              # the original -- so re-run the one failed check with -L to stream
              # the diagnostic (clippy lint / test output). nix does not cache
              # failures, so this just re-attempts that single check.
              try {
                ^nix build ...[
                  $inst
                  "-L"
                  "--no-link"
                  "--option" "accept-flake-config" "true"
                  "--option" "extra-experimental-features" "ca-derivations"
                ]
              } catch { }
            }
            print --stderr "::endgroup::"
          }
        }

        if $build_failed {
          exit 1
        }

        let tmp = (mktemp --directory --tmpdir "ix-check.XXXXXX")
        let report = ($tmp | path join "flake-schema-eval.jsonl")
        do --capture-errors {
          ^$eval_jobs ...[
            "--flake" ".#packages.x86_64-linux"
            "--workers" "16"
            "--gc-roots-dir" ($tmp | path join "flake-schema-eval-gc")
            "--option" "accept-flake-config" "true"
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
    '';
  };

  updateMods = ix.writePythonApplication pkgs {
    name = "update-mods";
    src = paths.tools.updateMods;
    pyChecker = "zuban";
    # pydantic validates Modrinth API responses at the boundary so upstream
    # drift fails with a path-precise error rather than a bare KeyError.
    python = pkgs.python314.withPackages (ps: [ ps.pydantic ]);
    meta.description = "Regenerate Minecraft mod catalogs";
  };

  updateLoaders = ix.writePythonApplication pkgs {
    name = "update-loaders";
    src = paths.tools.updateLoaders;
    pyChecker = "zuban";
    # pydantic validates the PaperMC fill v3 response at the boundary so upstream
    # drift fails with a path-precise error rather than a bare KeyError.
    python = pkgs.python314.withPackages (ps: [ ps.pydantic ]);
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

  # One symlink-free directory holding every skill under `skills/`, ready to
  # copy into `.claude/skills`.
  skillsDir = ix.skills.mkSkillsDir { inherit pkgs; };

  # The `index` Claude Code plugin: every index skill bundled for `--plugin-dir`,
  # invoked as `/index:<skill>`. This is the pure-index default (no hooks, no
  # personal skills); a consumer wanting extras calls `ix.claudePlugin.mkPlugin`
  # with `extraSkills`/`hooks` directly.
  claudePluginDir = ix.claudePlugin.mkPlugin {
    inherit pkgs;
    name = "index";
  };

  # Declarative subagents rendered to a symlink-free `.claude/agents` directory.
  # index-action-runner offloads a long, image- or step-heavy loop into its own
  # context and returns only the conclusion (ENG-2792). Its frontmatter bakes a
  # FRESH inline `index` server from the shared `ix.mcp` registry, so each
  # spawned subagent gets its own kernel and browser rather than sharing the
  # parent's; the server is declared from the same source the wrappers render.
  agentsDir =
    let
      # Agents whose frontmatter is computed in nix rather than written in the
      # file, so they are rendered (not copied verbatim) and their source `.md`
      # is body-only. index-action-runner offloads a long, image- or step-heavy
      # loop into its own context and returns only the conclusion (ENG-2792); its
      # frontmatter bakes a FRESH inline `index` server from the shared `ix.mcp`
      # registry, so each spawned subagent gets its own kernel and browser rather
      # than sharing the parent's, declared from the source the wrappers render.
      renderedAgents = {
        index-action-runner = {
          frontmatter = {
            name = "index-action-runner";
            description =
              "Offload a long, image-heavy or many-step loop (browser automation, "
              + "scanning many images or PDFs, multi-step web flows) into an isolated "
              + "context. Give it an outcome plus the exact fields to return; it drives "
              + "the whole loop in its own index kernel and returns only the distilled "
              + "result, keeping screenshots and DOM dumps out of the main thread.";
            mcpServers = ix.mcp.toAgentMcpServers {
              index = {
                transport = "stdio";
                command = lib.getExe repoPackages.mcp;
                args = [ "serve" ];
              };
            };
          };
          body = builtins.readFile (paths.agents + "/index-action-runner.md");
        };
      };
      # A rendered agent's source `.md` is body-only and excluded here by name;
      # every OTHER `agents/*.md` is a complete, hand-authored agent (frontmatter
      # + body) copied verbatim. A non-rendered file without leading `---` is a
      # mistake (a missing frontmatter block), so fail loudly with the offenders
      # rather than silently dropping it from the agent set.
      renderedFiles = map (n: "${n}.md") (builtins.attrNames renderedAgents);
      entries = builtins.readDir paths.agents;
      rawMdNames = lib.filter (
        n: lib.hasSuffix ".md" n && entries.${n} == "regular" && !(lib.elem n renderedFiles)
      ) (builtins.attrNames entries);
      missingFrontmatter = lib.filter (
        n: !(lib.hasPrefix "---" (builtins.readFile (paths.agents + "/${n}")))
      ) rawMdNames;
    in
    assert lib.assertMsg (missingFrontmatter == [ ])
      "agentsDir: agents/*.md without YAML frontmatter (add frontmatter, or render it in renderedAgents): ${lib.concatStringsSep ", " missingFrontmatter}";
    ix.agents.mkAgentsDir {
      inherit pkgs;
      agents = renderedAgents;
      rawFiles = map (n: {
        name = lib.removeSuffix ".md" n;
        path = paths.agents + "/${n}";
      }) rawMdNames;
    };

  mcSource = ix.writeNushellApplication pkgs {
    name = "mc-source";
    text = builtins.readFile paths.tools.mcSource;
    runtimeInputs = [
      (pkgs.callPackage packageRegistry.byId.vineflower.path { })
    ];
    meta.description = "Decompile a Minecraft server jar with Mojang mappings via Vineflower";
  };

  updateSounds = ix.writeNushellApplication pkgs {
    name = "update-sounds";
    text = builtins.readFile paths.tools.updateSounds;
    meta.description = "Refresh the pinned Minecraft sound pack in packages/minecraft/minecraft/sound";
  };

  benchFilesystem = import paths.bench.filesystem { inherit ix pkgs; };

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

  siteBuild = ix.buildSvelteSite pkgs {
    pname = "ix-site";
    version = "0.1.0";
    src = siteSrc;
    distDir = "build";
    serve = {
      name = "ix-site";
      routePrefix = "/index";
    };
    devServer = {
      name = "ix-site-dev";
      checkoutSubdir = "site";
    };
  };

  # The local preview serves the same `/index` build that Pages deploys.
  site = siteBuild.overrideAttrs (old: {
    passthru = (old.passthru or { }) // {
      preview = siteBuild.passthru.serve;
      static = siteBuild.passthru.staticSite;
    };
  });

  siteTests = ix.buildNpmVitest pkgs {
    pname = "ix-site";
    version = "0.1.0";
    src = siteSrc;
    preTest = ''
      node node_modules/@sveltejs/kit/src/cli.js sync
    '';
  };

  repoPackages = ix.packageSetFor pkgs;

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
  updaterFor =
    entry:
    let
      pkg =
        lib.attrByPath entry.packageSet.attrPath
          (throw "update: package `${entry.id}` is flagged `updateScript = true` but is absent from the package set for ${system}")
          repoPackages;
    in
    lib.getExe (
      pkg.updateScript
        or (throw "update: package `${entry.id}` is flagged `updateScript = true` but exposes no `passthru.updateScript`")
    );
  updateNodes = {
    mods.command = [ (lib.getExe updateMods) ];
    loaders.command = [ (lib.getExe updateLoaders) ];
    sounds.command = [ (lib.getExe updateSounds) ];
  }
  // lib.genAttrs' updatableEntries (
    entry: lib.nameValuePair entry.id { command = [ (updaterFor entry) ]; }
  );
  updateSpec = (pkgs.formats.json { }).generate "update-dag.json" { nodes = updateNodes; };
  update = ix.writeNushellApplication pkgs {
    name = "update";
    meta.description = "Refresh every repo content source (Minecraft catalogs + pinned binaries) in parallel via dag-runner";
    runtimeInputs = [ repoPackages.dag-runner ];
    text = ''
      def --wrapped main [...args] {
        exec dag-runner ...$args ${updateSpec}
      }
    '';
  };

  # Cross-compiled standalone binaries, exposed as `packages.<host>.<bin>-<triple>`.
  # Linux-only: the Apple (zig + macOS SDK) and musl cross toolchains run on a
  # Linux build host; an aarch64-darwin Mac builds Darwin targets natively and
  # cannot host the Linux→Darwin path, so gating here keeps darwin evaluation
  # from pulling an unbuildable graph. Start with `dag-runner` (pure Rust, no C
  # deps); widen `crossBinaries` as crates are confirmed cross-clean.
  # macOS targets only for now: the zig + SDK toolchain produces a working
  # linker out of the box. A musl target additionally needs a static musl
  # linker wrapper (clang + mold against a musl sysroot); until that lands,
  # `unitsFor` still accepts any triple but no musl package is exposed.
  crossTargets = [
    "aarch64-apple-darwin"
    "x86_64-apple-darwin"
  ];
  crossBinaries = [ "dag-runner" ];
  crossWorkspace = ix.rustWorkspaceFor pkgs;
  crossPackages = lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux (
    lib.mergeAttrsList (
      map (
        target:
        let
          units = crossWorkspace.unitsFor { inherit target; };
        in
        lib.genAttrs' crossBinaries (
          binary:
          lib.nameValuePair "${binary}-${target}" (
            units.binaries.${binary} or (throw "cross: workspace has no binary `${binary}` for ${target}")
          )
        )
      ) crossTargets
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

  rustPackageTestSets =
    let
      cargoUnit = ix.cargoUnitFor pkgs;
      rustWorkspace = ix.rustWorkspaceFor pkgs;
      # A crate with a `packageSet` is built through `repoPackages` and carries
      # its own `passthru.tests`. A lib-only workspace crate has no `packageSet`
      # and is not in `repoPackages`, so select its library straight from the
      # shared unit graph (same path ix-vt's default.nix uses). The library unit
      # key is the Cargo package name with dashes underscored.
      packageTestsFor =
        entry:
        if entry.packageSet != null then
          (lib.attrByPath entry.packageSet.attrPath
            (throw "packages/${entry.relativePath}/package.nix: passthruTests needs packageSet.attrPath")
            repoPackages
          ).passthru.tests or { }
        else
          (cargoUnit.selectLibraryWithTests rustWorkspace.units {
            library = lib.replaceStrings [ "-" ] [ "_" ] entry.id;
            packageName = entry.id;
          }).passthru.tests or { };
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
        ${prefix} = tests // {
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
      collectRust =
        group:
        lib.mergeAttrsList (
          map (entry: group entry.passthruTests.prefix (packageTestsFor entry)) repoEntries
          ++ lib.mapAttrsToList (
            packageName: package: group "rust-${packageName}" (package.passthru.tests or { })
          ) moduleRustPackages
        )
        // workspaceAuditTests;
    in
    {
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

  tests = import paths.tests { inherit nixpkgs ix paths; };

  exampleFleets = ix.exampleFleetsFor { hostSystem = system; };

  # Same fleets with "health-check-" prepended to every external name, so the
  # lifecycle scripts that force-delete VMs by name can never clobber an
  # unrelated production VM that happens to share the example's node name
  # (`nginx`, `factions`, ...). `withNodePrefix` only rewrites plan data, so
  # both surfaces share one NixOS closure evaluation per node instead of
  # evaluating every example fleet twice (ENG-2411).
  healthCheckExampleFleets = lib.mapAttrs (
    _name: fleet: fleet.withNodePrefix "health-check-"
  ) exampleFleets;

  # Surface every example's `ix fleet <sub>` wrapper as a flake package.
  # Each example contributes `packages.<system>.<example>-{up,health,...}`,
  # which lets `nix run .#nginx-lifecycle-up` invoke the existing fleet
  # plumbing through the wrapper's `meta.mainProgram`, and
  # `nix build .#nginx-lifecycle-up` produce the wrapper script on disk.
  examplePackages =
    let
      fleetSubs = [
        "up"
        "health"
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
          meta = (old.meta or { }) // {
            description = "Run `ix fleet ${sub}` against the ${name} example fleet";
          };
        });
      })
    ) exampleFleets;

  healthChecks =
    import ./image/health-checks.nix
      {
        inherit lib pkgs;
        inherit (ix) writeNushellApplication;
        dagRunner = repoPackages.dag-runner;
      }
      {
        exampleFleets = healthCheckExampleFleets;
        exampleNames = lib.attrNames exampleFleets;
      };

  # Shared between `packages` and the `image-<name>` checks so blast-radius
  # (which only diffs `.#ciChecks.x86_64-linux`) catches image fanouts via the
  # check drvPath shift. The `base` image is omitted: its config is just
  # `ix.image.name`/`tag`, and any base-profile change already fans out into
  # every discovered image.
  discoveredImages = ix.discoverImages {
    root = paths.images;
    inherit (tests) imageTests;
  };

  # Non-NixOS OCI example images (ubuntu, debian, ...). They live under
  # `examples/_non-nix-oci` so fleet discovery skips the subtree (leading
  # underscore), and are surfaced here as `non-nix-<name>` packages plus
  # `image-non-nix-<name>` checks, the same validation path discovered images
  # use. Each is imported with the example `{ index }` contract.
  nonNixExampleImages = lib.mapAttrs' (
    name: entry:
    lib.nameValuePair "non-nix-${name}" (
      import entry.path {
        index = {
          lib = ix;
        };
      }
    )
  ) (ix.discoverTree { root = paths.examples + "/_non-nix-oci"; });

  # The content-addressed `image.json` for each non-Nix example, surfaced as its
  # own package so the small artifact is buildable directly (`nix build
  # .#non-nix-ubuntu-description`) and cached independently of the materialized
  # tar it regenerates. See #679.
  nonNixExampleDescriptions = lib.mapAttrs' (
    name: image: lib.nameValuePair "${name}-description" image.passthru.description
  ) nonNixExampleImages;

  # Build the check catalog from a rust-package keying. `checks` (flat: one
  # derivation per `checks.<system>.<name>`, required by the flake schema and
  # `nix flake check`) and `ciChecks` (sharded: one `recurseForDerivations` group
  # per package, what the memory-bounded CI evaluator consumes) share the same
  # explicit and image checks; only the rust keying differs (ENG-2201). The
  # collision guard runs per keying, so producing `ciChecks` only forces the
  # cheap per-package names, never the flat per-#[test] spine.
  catalogFor =
    rustPackageSet:
    lib.optionalAttrs (system == ix.system) (
      let
        rustChecks = {
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
          # Boots a NixOS VM running the minecraft-blocks producer's Paper
          # server and asserts the BlockEvents plugin's onEnable succeeded
          # with no exception (ENG-2186). Paper's paperclip bootstrap is
          # pre-run at build time so the VM never needs the network; see
          # tests/minecraft-blocks-vm.nix.
          minecraft-blocks-vm = tests.minecraftBlocksVm;
          # Skills and subagents are not committed; they are rendered live by the
          # SessionStart hook. This gate forces the skills directory and the
          # subagents directory (both of which evaluate the no-symlink
          # materialization check) to build.
          agent-skills = pkgs.runCommand "agent-skills-check" { } ''
            test -d ${skillsDir}
            test -d ${agentsDir}
            mkdir -p "$out"
          '';
          # Pins the last-applied 3-way merge behind homeModules.mutable-json:
          # first-install, preserve an app-written key, enforce a key the app
          # changed, prune a key Nix stopped declaring, and keep a sibling key
          # while a declared array is replaced atomically.
          mutable-json-merge =
            pkgs.runCommand "mutable-json-merge-check" { nativeBuildInputs = [ pkgs.jq ]; }
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
          # Offline schema gate for the loader manifests. `deepSeq` forces
          # every Paper / Velocity / Fabric per-version lock through
          # `readLoaderManifest` in `lib/artifacts.nix`, so malformed JSON or a
          # missing key fires here before any image starts evaluating. The
          # forced surface is the parsed-and-validated manifest data, not the
          # wrapped `fetchurl` derivations, to keep this check pure eval.
          loader-manifests =
            let
              forced = builtins.deepSeq ix.artifacts.minecraft.loaderManifests "ok";
            in
            pkgs.runCommand "loader-manifests-check" { } ''
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
          # stages (nixfmt / statix / deadnix / astlog itself) never scan the
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
                nativeBuildInputs = [ repoPackages.scipql ];
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
          # Symphony's required quality lane (compile -Werror, mix format,
          # `mix credo --strict`, mix test), built through the shared
          # ix.buildElixirCheck lane against the repo-wide strict Credo config
          # (lib/elixir/credo.exs); see packages/agent/symphony/default.nix. The
          # advisory lane (dialyzer, sobelow, deps.audit) stays a local
          # `mix quality` run.
          symphony-elixir = repoPackages.symphony.passthru.tests.elixir;
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
          lint = pkgs.runCommand "ix-images-lint" { nativeBuildInputs = [ pkgs.coreutils ]; } ''
            cp -R ${lintSource} source
            chmod -R u+w source
            cd source
            ${lib.getExe lint}
            mkdir -p "$out"
          '';
          # Exercises the trusted half of the blast-radius PR comment: the
          # validate/render jq embedded in its workflow, extracted from the YAML so
          # the test can't drift from what the trusted comment job runs. The
          # report-building logic lives in the `blast-radius` Rust crate and is
          # covered by its own unit tests. See tools/blast-radius-test.sh.
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
                bash tools/blast-radius-test.sh
                mkdir -p "$out"
              '';
          # Proves the Linux→macOS cross toolchain actually emits a Darwin object,
          # which a successful build alone does not assert. `file` reads the Mach-O
          # header; a regression in the zig/SDK wiring fails here on x86_64-linux CI
          # rather than silently shipping a wrong-arch binary.
          cross-darwin-smoke = pkgs.runCommand "cross-darwin-smoke" { nativeBuildInputs = [ pkgs.file ]; } ''
            bin=${crossPackages."dag-runner-aarch64-apple-darwin"}/bin/dag-runner
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
          site-case-tests = pkgs.linkFarm "site-case-tests" (
            lib.mapAttrsToList (name: path: { inherit name path; }) siteTests.cases
          );
          site-test = siteTests.all;
        };
        # One check per image, built by nix-fast-build in step 1 of `.#check`.
        # Without this, `.#packages` carries the image derivations but `.#checks`
        # does not, so blast-radius under-reports any change that rebuilds every
        # image because the drvPath shift never reaches a check. The `eval`
        # aggregate at tests/default.nix:3890 only closes over per-image
        # `extraScript` text, not `config.system.build.toplevel`, so it stays
        # stable across semantic edits to shared image libs.
        #
        # For Nix images the check builds the system *closure*
        # (`passthru.toplevel`), not the OCI tar the package output is. Packing
        # the closure into a layered, compressed OCI archive
        # (`streamLayeredImage`) is ~60-100s of deterministic tar+compress per
        # image, and the gate consumes none of those bytes: it only needs "this
        # image's closure builds", and the archive is rebuilt at release where a
        # registry push actually uploads the layers. Gating on the closure keeps
        # that signal (and gives blast-radius the toplevel drvPath directly)
        # while dropping the tar pass, which dominated CI wall-clock because the
        # closure includes frequently-changing packages (e.g. the base-profile
        # mcp), so any edit re-packed all ~15 archives. Non-Nix example images
        # (`mkNonNixImage`: a pulled Debian/Ubuntu base) have no Nix toplevel, so
        # their check stays the assembled archive.
        imageChecks =
          lib.mapAttrs' (n: v: lib.nameValuePair "image-${n}" v.toplevel) discoveredImages
          // lib.mapAttrs' (n: v: lib.nameValuePair "image-${n}" v) nonNixExampleImages;
        # Rust crate prefixes can be overridden in `package.nix` and image
        # names are user-chosen, so a stray collision with an explicit check
        # would otherwise be silently swallowed by the `//` merge. Two pairwise
        # intersections cover all three pairs because every offending name is
        # in at least two sets.
        checkNameCollisions =
          lib.intersectLists (lib.attrNames explicitChecks) (lib.attrNames rustChecks)
          ++ lib.intersectLists (lib.attrNames imageChecks) (lib.attrNames (explicitChecks // rustChecks));
      in
      assert lib.assertMsg (checkNameCollisions == [ ])
        "checks: duplicate names across explicit/rust/image sets: ${lib.concatStringsSep ", " checkNameCollisions}";
      explicitChecks // rustChecks // imageChecks
    );
in
{
  packages =
    discoveredImages
    // {
      base =
        let
          package = ix.mkImage {
            modules = [
              {
                ix.image = {
                  name = "ix/base";
                  tag = "latest";
                };
              }
            ];
          };
        in
        package
        // {
          passthru = (package.passthru or { }) // {
            tests = (package.passthru.tests or { }) // {
              eval = tests.imageTests.base;
            };
          };
        };
      health-checks = healthChecks.dag;
      health-checks-zellij = healthChecks.zellij;
      inherit check lint site;
      site-dev = site.passthru.devServer;
      bench-filesystem = benchFilesystem;
      update-mods = updateMods;
      update-loaders = updateLoaders;
      inherit update;
      ix-shell-sync-ignored = ixShellSyncIgnored;
      mc-source = mcSource;
      update-sounds = updateSounds;
      agents = agentsDir;
      skills = skillsDir;
      claude-plugin = claudePluginDir;
    }
    // repoFlakePackages
    // examplePackages
    // nonNixExampleImages
    // nonNixExampleDescriptions
    // crossPackages
    // healthChecks.lifecyclePackages;

  # Flat keying: one derivation per `checks.<system>.<name>`, as the flake schema
  # and `nix flake check` require. The `.#check` gate and blast-radius consume
  # the sharded `ciChecks` instead, so this output is not what CI enumerates.
  checks = catalogFor rustPackageTestSets.flat;
  # Sharded keying for the memory-bounded CI evaluator (nix-fast-build /
  # nix-eval-jobs / blast-radius): each package's per-#[test] checks sit under one
  # `recurseForDerivations` group, so the evaluator lists cheap per-package names
  # at the root and forces each crate's manifest IFD in its own worker job
  # (ENG-2201). Not a `checks.<system>.<name>` output, because a non-derivation
  # there fails the flake schema.
  ciChecks = catalogFor rustPackageTestSets.sharded;

  formatter = pkgs.nixfmt;

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
        pkgs.nixfmt
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

    # Dev loop for packages/symphony: the Elixir/OTP pairing the runtime pins
    # (1.19 on 28) plus the host tools bin/run-nix expects. codex is the plain
    # nixpkgs CLI; authenticate it before `nix run .#symphony`.
    symphony = pkgs.mkShellNoCC {
      packages = [
        (ix.languages.elixir.toolchain pkgs { version = "1.19"; })
        (ix.languages.erlang.toolchain pkgs { version = "28"; })
        pkgs.codex
        pkgs.gh
        pkgs.git
        pkgs.openssh
      ];
    };
  };
}
