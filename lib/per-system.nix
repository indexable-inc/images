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
  };

  # Each lint stage is one subcommand on a single binary so the spec keys
  # off `lib.getExe lintStage` without registering four sibling packages.
  # The Nu wrapper checks syntax at build time, so a typo in a stage shows
  # up in the `lint` derivation build, not at `nix run` time.
  lintStage = ix.writeNushellApplication pkgs {
    name = "lint-stage";
    meta.description = "One lint stage (nixfmt | statix | deadnix | ast-grep | ast-grep-test); driven by `lint`";
    runtimeInputs = [
      pkgs.ast-grep
      pkgs.deadnix
      pkgs.fd
      pkgs.nixfmt
      pkgs.statix
    ];
    text = ''
      def "main nixfmt" [] {
        let nix_files = (fd --extension nix | lines)
        nixfmt --check ...$nix_files
      }
      def "main statix" [] { statix check --ignore '.claude/worktrees' . }
      def "main deadnix" [] { deadnix --fail --no-lambda-pattern-names . }
      def "main ast-grep" [] { ast-grep scan --error . }
      # Rule self-test: every fixture under nix-rules-tests must flag its
      # invalid cases and ignore its valid ones. Catches rules whose pattern
      # silently stops matching (e.g. a bare `attr = val` that parses as an
      # expression, not a binding). --skip-snapshot-tests keeps it to match
      # presence/absence without baseline snapshot files.
      def "main ast-grep-test" [] { ast-grep test --skip-snapshot-tests }
      def main [] {
        error make { msg: "specify a stage: nixfmt | statix | deadnix | ast-grep | ast-grep-test" }
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
      "ast-grep".command = [
        (lib.getExe lintStage)
        "ast-grep"
      ];
      "ast-grep-test".command = [
        (lib.getExe lintStage)
        "ast-grep-test"
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
  # Step 1 (nix-fast-build) builds every `checks.x86_64-linux` derivation: it
  # evaluates with nix-eval-jobs (parallel) and streams each drv into a build
  # pool as it resolves. --skip-cached drops paths already in a substituter (a
  # warm run does almost no work), --no-nom keeps plain logs, --no-link leaves no
  # result symlinks. It exits nonzero iff a build or eval fails: that is the gate.
  # --eval-workers 16 with --eval-max-memory-size 6144 is a headroom guard rail
  # (above nix-eval-jobs' 4 GiB default per worker, below the old 8 GiB), not a
  # workaround: the per-crate check split (see the `checks` block below) keeps
  # each worker's eval bounded by the largest single crate. Pinned by commit to
  # nix-fast-build 1.5.0.
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
  # `set -o pipefail`). Pinned by commit to nix-eval-jobs v2.34.1, matching the
  # host Nix 2.34.x.
  check = ix.writeNushellApplication pkgs {
    name = "check";
    meta.description = "Run the full CI gate: build .#checks.x86_64-linux and eval-validate .#packages.x86_64-linux";
    text = ''
      const fast_build = "github:Mic92/nix-fast-build/7f185e0ec37b65b4730f892e0de9a831b0610f3a"
      const eval_jobs = "github:nix-community/nix-eval-jobs/65ebf5b7cd453a27af09cf02b1fc57b3568cc4b7"

      def main [] {
        ^nix run $fast_build -- ...[
          "--flake" ".#checks.x86_64-linux"
          "--eval-max-memory-size" "6144"
          "--eval-workers" "16"
          "--skip-cached"
          "--no-nom"
          "--no-link"
          "--option" "accept-flake-config" "true"
        ]

        let tmp = (mktemp --directory --tmpdir "ix-check.XXXXXX")
        let report = ($tmp | path join "flake-schema-eval.jsonl")
        do --capture-errors {
          ^nix run $eval_jobs -- ...[
            "--flake" ".#packages.x86_64-linux"
            "--workers" "16"
            "--gc-roots-dir" ($tmp | path join "flake-schema-eval-gc")
            "--option" "accept-flake-config" "true"
            "--option" "eval-cache" "false"
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
    meta.description = "Regenerate Minecraft mod catalogs";
  };

  updateLoaders = ix.writePythonApplication pkgs {
    name = "update-loaders";
    src = paths.tools.updateLoaders;
    meta.description = "Refresh Minecraft loader (Paper / Velocity / Fabric) catalogs from upstream";
  };

  updateIxCli = ix.writePythonApplication pkgs {
    name = "update-ix-cli";
    src = paths.tools.updateIxCli;
    runtimeInputs = [ pkgs.nix ];
    meta.description = "Re-prefetch the ix.dev CLI binaries and bump packages/ix/default.nix hashes";
  };

  ixShellSyncIgnored = ix.writePythonApplication pkgs {
    name = "ix-shell-sync-ignored";
    src = paths.tools.ixShellSyncIgnored;
    runtimeInputs = [
      pkgs.git
      pkgs.gnutar
    ];
    meta.description = "Copy git-ignored files into an ix shell workspace";
  };

  # Always-on instruction documents. Forcing either string evaluates the
  # `agent-context` always-on cap assertion (see lib/agent-context.nix).
  agentContextClaudeMd = pkgs.writeText "CLAUDE.md" ix.agentContext.alwaysDoc;
  agentContextCodexMd = pkgs.writeText "AGENTS.md" ix.agentContext.alwaysDoc;

  # One link farm holding every handwritten skill plus one generated skill per
  # `disclosure: progressive` section, ready to symlink into `.claude/skills`.
  agentContextProgressiveSkills = ix.agentContext.mkProgressiveSkills { inherit pkgs; };
  agentContextSkillCollisions = lib.intersectLists ix.skills.allSkills (
    lib.attrNames agentContextProgressiveSkills
  );
  agentContextSkills =
    assert lib.assertMsg (agentContextSkillCollisions == [ ])
      "agent-context: progressive section names collide with handwritten skills: ${lib.concatStringsSep ", " agentContextSkillCollisions}";
    ix.skills.mkSkillsDir {
      inherit pkgs;
      extraSkills = agentContextProgressiveSkills;
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
    meta.description = "Refresh the pinned Minecraft sound pack in packages/minecraft/sound";
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

  siteSrc = fs.toSource {
    root = paths.site;
    fileset = fs.intersection (fs.gitTracked paths.site) (
      fs.unions [
        (paths.site + "/package.json")
        (paths.site + "/package-lock.json")
        (paths.site + "/mdsvex.config.js")
        (paths.site + "/svelte.config.js")
        (paths.site + "/vite.config.ts")
        (paths.site + "/vitest.config.ts")
        (paths.site + "/tsconfig.json")
        (paths.site + "/eslint.config.js")
        (paths.site + "/src")
        (paths.site + "/static")
      ]
    );
  };

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
        lib.listToAttrs (
          map (
            binary:
            lib.nameValuePair "${binary}-${target}" (
              units.binaries.${binary} or (throw "cross: workspace has no binary `${binary}` for ${target}")
            )
          ) crossBinaries
        )
      ) crossTargets
    )
  );

  repoFlakePackages = lib.listToAttrs (
    map (
      entry:
      lib.nameValuePair entry.flake.attrName (
        lib.attrByPath entry.packageSet.attrPath
          (throw "packages/${entry.relativePath}/package.nix: flake output `${entry.flake.attrName}` needs packageSet.attrPath")
          repoPackages
      )
    ) (packageRegistry.flakeEntriesFor system)
  );

  rustPackageTests =
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
      repoRustPackageTests = lib.mergeAttrsList (
        map (
          entry:
          lib.mapAttrs' (testName: test: lib.nameValuePair "${entry.passthruTests.prefix}-${testName}" test) (
            packageTestsFor entry
          )
        ) (packageRegistry.passthruTestEntriesFor system)
      );
      moduleRustPackages = {
        resource-monitor-stats-writer = cargoUnit.selectBinaryWithTests rustWorkspace.units {
          binary = "resource-monitor-stats-writer";
        };
      };
      moduleRustPackageTests = lib.concatMapAttrs (
        packageName: package:
        lib.mapAttrs' (testName: test: lib.nameValuePair "rust-${packageName}-${testName}" test) (
          package.passthru.tests or { }
        )
      ) moduleRustPackages;
    in
    repoRustPackageTests // moduleRustPackageTests;

  lintSource = fs.toSource {
    inherit (paths) root;
    fileset = fs.gitTracked paths.root;
  };

  tests = import paths.tests { inherit nixpkgs ix; };

  exampleFleets = ix.exampleFleetsFor { hostSystem = system; };

  # Separate aggregation with "health-check-" prepended to every node name,
  # so the lifecycle scripts that force-delete VMs by name can never clobber
  # an unrelated production VM that happens to share the example's node name
  # (`nginx`, `factions`, ...).
  healthCheckExampleFleets = ix.exampleFleetsFor {
    hostSystem = system;
    nodePrefix = "health-check-";
  };

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
      lib.listToAttrs (
        map (sub: {
          name = "${name}-${sub}";
          value = fleet.${sub}.overrideAttrs (old: {
            meta = (old.meta or { }) // {
              description = "Run `ix fleet ${sub}` against the ${name} example fleet";
            };
          });
        }) fleetSubs
      )
    ) exampleFleets;

  healthChecks =
    import ./health-checks.nix
      {
        inherit lib pkgs;
        inherit (ix) writeNushellApplication;
        dagRunner = repoPackages.dag-runner;
      }
      {
        exampleFleets = healthCheckExampleFleets;
        exampleNames = lib.attrNames exampleFleets;
      };
in
{
  packages =
    (ix.discoverImages {
      root = paths.images;
      inherit (tests) imageTests;
    })
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
      update-ix-cli = updateIxCli;
      ix-shell-sync-ignored = ixShellSyncIgnored;
      mc-source = mcSource;
      update-sounds = updateSounds;
      claude-md = agentContextClaudeMd;
      codex-md = agentContextCodexMd;
      skills = agentContextSkills;
    }
    // repoFlakePackages
    // examplePackages
    // crossPackages
    // healthChecks.lifecyclePackages;

  checks = lib.optionalAttrs (system == ix.system) (
    let
      # Each per-crate rust test unit is its own top-level check (spread into
      # the `checks` set below) rather than being collapsed into one aggregate
      # linkFarm. That lets nix-eval-jobs (the evaluator CI's nix-fast-build
      # wraps) assign each crate's test to a separate worker, so no single
      # worker holds the whole workspace test graph in its heap. The old
      # `rust-package-tests` linkFarm did the opposite: it forced one worker to
      # evaluate every crate at once, and that single multi-GiB eval is what
      # flaked the flake-check job (per-worker SIGKILL at the memory cap, and
      # host-OOM with many workers).
      rustChecks = {
        cargo-unit-real-workspaces = tests.cargoUnitRealWorkspaces;
      }
      // rustPackageTests;
      explicitChecks = {
        inherit (tests) eval;
        # Instruction files are not committed; they are rendered live by the
        # SessionStart hook. This gate forces the rendered always-on documents
        # (which evaluates the always-on char cap assertion) and the combined
        # skills link farm (which evaluates the name-collision assertion) to build.
        agent-context = pkgs.runCommand "agent-context-check" { } ''
          test -s ${agentContextClaudeMd}
          test -s ${agentContextCodexMd}
          test -d ${agentContextSkills}
          mkdir -p "$out"
        '';
        # Pins the last-applied 3-way merge behind homeModules.mutable-json:
        # first-install, preserve an app-written key, enforce a key the app
        # changed, prune a key Nix stopped declaring, and keep a sibling key
        # while a declared array is replaced atomically.
        mutable-json-merge =
          pkgs.runCommand "mutable-json-merge-check" { nativeBuildInputs = [ pkgs.jaq ]; }
            ''
              prog=${ix.mutableJson.mergeProgram}
              run() { jaq -ncS --argjson last "$1" --argjson live "$2" --argjson new "$3" -f "$prog"; }
              check() {
                expected=$(printf '%s' "$2" | jaq -cS .)
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
        run-records-session = repoPackages.run.passthru.tests.recordsSession;
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
      # A rust check key is `<prefix>-<testName>`, where `prefix` defaults to
      # `rust-<id>` but a crate may override it in its `package.nix` (e.g.
      # `ix-fleet` uses `ix-fleet`). So the rust keys are not guaranteed to be
      # `rust-`-prefixed, and a stray override colliding with an explicit check
      # name would otherwise be silently swallowed by the `//` merge. Assert the
      # two key sets are disjoint so such a collision fails loudly instead.
      # Forcing `rustChecks`' keys realizes the cargo-unit test manifest (IFD);
      # that is acceptable because evaluating the `checks` set at all (what CI
      # and `nix flake check` do) already enumerates those keys.
      checkNameCollisions = lib.intersectLists (lib.attrNames explicitChecks) (lib.attrNames rustChecks);
    in
    assert lib.assertMsg (checkNameCollisions == [ ])
      "checks: rust check name(s) collide with explicit checks: ${lib.concatStringsSep ", " checkNameCollisions}";
    explicitChecks // rustChecks
  );

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
