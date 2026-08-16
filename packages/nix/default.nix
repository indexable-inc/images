{
  ix,
  lib,
  updateScriptWriter ? null,
  # Link the in-tree Rust evaluator into the `nix` CLI, so `eval-backend =
  # rust` and `eval-backend = shadow` have a backend to route to. Off by
  # default because it adds a Rust toolchain and a vendored cargo registry to
  # every host that builds this package; hydra turns it on to dogfood the
  # evaluator under `shadow`, which serves the C++ answer and only records
  # what the Rust arm got wrong.
  withRustEval ? false,
}:
# The Nix view is surfaced as `ix.nixSrc` and built through nixpkgs' own
# modular nix packaging so the result is a protocol-compatible drop-in for the
# 2.34.7 daemon the fleet runs.
#
# The source is used as it comes. There is no in-repo series and no `./patches`
# directory. An earlier revision of this file described one, from a de-forking
# attempt that was reverted, and the description outlived the mechanism.
#
# So the fork's delta is not enumerated here, deliberately: it is the commits
# between the upstream anchor and the view tip. A list here would be a second
# copy that drifts. What the delta currently carries, in one line each: the
# GC-roots client-interrupt daemon crash fix; treating an inaccessible default
# lookup-path entry as absent (the macOS sandbox denies the host's
# root-channels dir with EPERM, which aborted the C API unit tests and the
# recursive-nix functional test on any darwin host that has one, while clean CI
# builders lack the path and so carry the stock drvs green); the
# `build-status-dir` build-observability series, where behind an experimental
# feature of that name every active build or substitution goal writes a JSON
# status file under `<nixStateDir>/status/`, readable daemonlessly via `nix
# store builds [--json]`; the lazy trees backport (NixOS/nix#15711 and its
# post-merge fixes) behind an off-by-default `lazy-trees` eval setting
# (indexable-inc/index#3645, and see indexable-inc/index#4297 for why no host
# sets it); `builtins.wasm` from the open upstream PR NixOS/nix#15380 behind
# `wasm-builtin`, with deterministic execution forced so eval stays
# bit-identical across the mixed fleet (indexable-inc/index#3997); lazy git ref
# resolution so rev-pinned `builtins.fetchGit` inputs evaluate without network
# once cached (indexable-inc/index#4028); a jj working-copy fetcher; and an
# in-process parallel evaluator behind an off-by-default `eval-cores`, which
# also moved where an infinite recursion is reported (see the fork's release
# notes). The fork's `codex/flake-check-eval-cache` branch (draft PR
# indexable-inc/nix#1) is deliberately excluded: self-declared WIP, untested,
# incomplete.
let
  # Read `pkgs` from `ix` rather than a `pkgs` callPackage formal: a `src`/`pkgs`
  # formal is fragile against `callPackage` auto-binding, and the rest of the
  # nix/* packages read `pkgs` off their argument the same way.
  inherit (ix) pkgs;

  # Cross lane (RFC 0009, #3585): when the registry cross lane instantiates
  # this package (`cross = true` in package.nix), swap the component scope to
  # the Linux -> Darwin nixpkgs cross scope so the whole modular C++ closure
  # builds on the linux fleet and a Mac only substitutes the fork daemon.
  # Everything modular below reads `componentPkgs`; `pkgs` stays the native
  # package set for build-platform helpers (provenance JSON, update script,
  # test tooling). See lib/darwin/nixpkgs-cross.nix for the scope.
  isCross = ix.cross.isCross or false;
  componentPkgs =
    if isCross
    then ix.cross.pkgs
    else pkgs;
  # `file -bL` prints the architecture in Mach-O spelling.
  machoArch =
    if lib.hasPrefix "aarch64-" (ix.cross.target or "")
    then "arm64"
    else "x86_64";

  bootstrapLockPath = ix.paths.root + "/.github/actions/bootstrap-patched-nix/lock.json";
  bootstrapLock = lib.importJSON bootstrapLockPath;
  updateScriptArgs = {
    name = "nix-ix-bootstrap-lock-update";
    runtimeInputs = [
      pkgs.git
    ];
    meta.description = "Record the checked Nix view bootstrap output";
    text = ''
      # nu
      const lock_path = ".github/actions/bootstrap-patched-nix/lock.json"
      const source_path = "views/nix"

      def main [] {
        let current = (open $lock_path)
        let actual_system = (
          ^nix --extra-experimental-features nix-command eval --raw --expr builtins.currentSystem
          | str trim
        )
        if $actual_system != $current.system {
          error make {msg: $"bootstrap lock requires ($current.system), found ($actual_system)"}
        }
        let source_tree = (^git rev-parse $"HEAD:($source_path)" | str trim)
        let output_path = (
          ^nix build --no-link --print-out-paths $"path:./($source_path)#nix-cli"
          | str trim
        )
        {outputPath: $output_path, sourceTree: $source_tree, system: $actual_system}
        | to json --indent 2
        | save --force $lock_path
        print $"updated ($lock_path) to ($source_tree)"
      }
    '';
  };

  # nixpkgs builds `nixVersions.nix_2_34` as
  # `(nixComponents_2_34.overrideSource fetchedSrc).appendPatches patches_common`
  # then takes `.nix-everything` (pkgs/tools/package-management/nix/default.nix).
  # The pinned rev IS the tag 2.34.7 nixpkgs itself fetches (byte-identical
  # narHash), so swapping the fetched source for our patched tree via the same
  # modular `overrideSource` handle rebuilds every component from the patched
  # tree while keeping nixpkgs' interdependency scope and build wiring intact.
  base = componentPkgs.nixVersions.nixComponents_2_34;
  upstreamVersion = lib.removeSuffix "\n" (builtins.readFile (ix.nixSrc + "/.version"));

  # curl 8.21.0 started consuming the public curl_multi_wakeup() eventfd from
  # inside curl_multi_perform(). That loses a wakeup for callers which perform
  # before polling, and libstore's file-transfer worker is exactly such a
  # caller: it can then sleep for its full 10-second idle timeout. Upstream
  # fixed it by giving the threaded resolver a separate internal wakeup pair
  # (https://github.com/curl/curl/issues/22272).
  #
  # This source override belongs here instead of on `pkgs.curl`. Curl reaches
  # GHC, rustc, cargo and the whole python package set as a build input via
  # git-minimal (ghc -> sphinx -> pytest-xdist -> execnet -> hatch-vcs -> git-minimal ->
  # curl), so overriding it globally rehashes most of nixpkgs and detaches the
  # tree from cache.nixos.org. Measured on 2026-07-25 against nixpkgs
  # e2587cae: arrow-cpp substitutes as a 28.5 MiB download and souffle as
  # 2.8 MiB, and both were being compiled from source here for this one patch.
  # `modular/src/libstore/package.nix` is the only nix component that takes
  # curl as an input, so scoping it to that component keeps the fix where the
  # stall happens and leaves everything else matching the binary cache.
  #
  # The view is a Git tree, so it lacks the generated files in Curl's release
  # tarball. Regenerating them adds an autotools step to this scoped build.
  # Drop this override once nixpkgs ships a Curl containing
  # 009fd378e8f01c97ebe67a14a41a06d56430f3df. The version assertion makes a
  # nixpkgs Curl bump fail visibly instead of silently carrying a stale fork.
  curlForNixStore = assert lib.assertMsg (componentPkgs.curl.version == "8.21.0")
  "remove the Curl source override: expected nixpkgs Curl 8.21.0, got ${componentPkgs.curl.version}";
    componentPkgs.curl.overrideAttrs (old: {
      src = ix.curlSrc;
      nativeBuildInputs = (old.nativeBuildInputs or []) ++ [componentPkgs.autoreconfHook];
      postPatch = ''
        # shell
        patchShebangs scripts
      '';
      preConfigure = ''
        # shell
        substituteInPlace ./config.guess --replace-fail /usr/bin/uname uname
      '';
    });

  # The source is the complete Nix view tree. The view history carries the
  # changes on top of upstream 2.34.7, so the only remaining build-time
  # patches are nixpkgs' own (`patchesCommon`).
  #
  # Identify a patched daemon by version: `nix --version` (and
  # `builtins.nixVersion`) report the version each *component* was compiled
  # with -- the modular build's preConfigure writes the component derivation's
  # `version` into the tree's `.version` on every build, so a `.version` source
  # patch in our series would be clobbered and a version override on the
  # `nix-everything` aggregate would only rename the store path. Set it through
  # `overrideAllMesonComponents`, the last layer in the component builder
  # stack. The marker is semver build metadata (`+ix.g<rev12>.h<hash>`), not
  # `-ix`: meson feeds the version to darwin ld's -current_version, which
  # rejects a `-` suffix as a "malformed 32-bit x.y.z version number" but
  # tolerates `+`.
  patchedNix = let
    # nixpkgs' modular components derive sourceRoot from `patchedSrc.name`.
    # The checked view is a path whose unpacked directory is `nix-source`, so
    # wrap that path with the one field the nixpkgs boundary requires.
    patchedSrc = {
      name = "nix-source";
      outPath = ix.nixSrc;
    };
    source = {
      version = upstreamVersion;
      storePath = builtins.unsafeDiscardStringContext (toString ix.nixSrc);
    };
    commonPatches = [];
    sourceDigest = builtins.hashString "sha256" (builtins.toJSON {
      inherit (source) storePath;
      inherit commonPatches;
    });
    shortHash = builtins.substring 0 20 sourceDigest;
    version = "${upstreamVersion}+ix.h${builtins.substring 0 8 sourceDigest}";
    provenance = {
      schema = 3;
      algorithm = "sha256";
      inherit commonPatches sourceDigest source version;
    };
    provenanceJson = (pkgs.formats.json {}).generate "nix-ix-provenance.json" provenance;

    # The Rust evaluator, compiled in its own derivation and handed to the CLI
    # as a prebuilt archive rather than built from inside the nix-cli build.
    #
    # It has to be done this way here. nixpkgs' modular packaging vendors its
    # OWN src/nix/package.nix, so the fileset in the fork's copy -- the thing
    # that would carry ../../rust and the derivation.nix that build.rs
    # fingerprints -- never applies on this path. Same trap as the `wasm`
    # feature above, whose in-tree dependency declaration is likewise ignored.
    # Here the fileset is ours, so both inputs are easy to include.
    #
    # `nix build path:./views/nix#nix-cli --arg withRustEval true` still takes
    # the in-tree cargo path; this prefix only overrides it for this build.
    nixEvalRs = componentPkgs.stdenv.mkDerivation {
      pname = "nix-eval-rs";
      inherit version;

      # The whole patched tree, not a fileset over it. `ix.nixSrc` is a
      # string-like store path rather than a path, which `lib.fileset` rejects
      # outright -- and narrowing would buy nothing anyway: this derivation is
      # keyed on `ix.nixSrc`, so it already rebuilds exactly when the fork
      # moves. Taking the tree whole also keeps
      # src/libexpr/primops/derivation.nix in reach, which build.rs hashes to
      # key the evaluator's compiler fingerprint
      # (rust/nix-eval-rs/compiler-fingerprint.rs) and without which the crate
      # fails in the build script before compiling anything.
      src = ix.nixSrc;

      nativeBuildInputs = [
        componentPkgs.cargo
        componentPkgs.rustc
        componentPkgs.rustPlatform.cargoSetupHook
        # Do the cargo invocation through nixpkgs' hook rather than by hand.
        # `componentPkgs` is a CROSS set -- built on x86_64-linux, hosted on
        # aarch64-apple-darwin -- and a bare `cargo build` targets the
        # *builder*, so cargo compiles for x86_64-linux while stdenv's CC is
        # the darwin cross compiler. That combination fed `--target
        # x86_64-unknown-linux-gnu` to a darwin clang wrapper and died on a
        # missing wchar.h; had it linked, it would have produced a Linux
        # archive for a Darwin binary. The hook sets the six things that must
        # agree: --target, CARGO_BUILD_TARGET, CC_<TRIPLE>, CXX_<TRIPLE>,
        # CARGO_TARGET_<TRIPLE>_LINKER, and HOST_CC/HOST_CXX (build-platform
        # compilers, kept distinct from the host-platform ones).
        componentPkgs.rustPlatform.cargoBuildHook
      ];

      # Vendored so cargo never reaches the network. The lock pins rnix to a
      # git rev (a fork adding 1_000-style digit separators), which
      # importCargoLock cannot hash on its own.
      cargoDeps = componentPkgs.rustPlatform.importCargoLock {
        lockFile = ix.nixSrc + "/rust/Cargo.lock";
        outputHashes = {
          "rnix-0.12.0" = "sha256-CEBnghY4vr+FTR0d7tUkdjrgXgPtws+EA+Ig8aOM904=";
        };
      };
      cargoRoot = "rust";
      # The hook installs itself as buildPhase only when buildPhase is unset,
      # so there is deliberately no buildPhase here. It cds into this subdir
      # and pins CARGO_TARGET_DIR to <sourceRoot>/target.
      buildAndTestSubdir = "rust";
      cargoBuildFlags = ["-p" "nix-eval-rs"];
      # buildRustPackage would default this; plain mkDerivation does not, and
      # the hook interpolates it unguarded -- `--profile ""` and a
      # CARGO_PROFILE__STRIP with an empty infix. Must be set explicitly.
      cargoBuildType = "release";

      installPhase = ''
        # shell
        runHook preInstall
        mkdir -p "$out/lib" "$out/include"
        # Cargo emits into <target-dir>/<triple>/release whenever --target is
        # passed, which the hook always does. Take the triple from nix rather
        # than from $CARGO_BUILD_TARGET: the hook exports that only for the
        # cargo process itself, so it is not in scope here.
        cp "target/${componentPkgs.stdenv.hostPlatform.rust.cargoShortTarget}/release/libnix_eval_rs.a" "$out/lib/"
        cp rust/nix-eval-rs/include/*.h "$out/include/"
        runHook postInstall
      '';
    };
    patchedComponents = ((base.overrideSource patchedSrc).overrideAllMesonComponents
      (_: _: {inherit version;}))
      .overrideScope (_: prev: {
      # Patch 0038 (builtins.wasm, NixOS/nix#15380) adds a `wasm` meson
      # feature to libexpr and declares its wasmtime dependency in the
      # in-tree src/libexpr/package.nix; the nixpkgs modular scope builds
      # from its own vendored component packaging, so that declaration
      # never reaches this build. Worse, nixpkgs' meson hook passes
      # --auto-features=enabled, which flips the feature on with nobody
      # supplying the library (wasm.cc fails on #include <wasmtime.hh>).
      # Wire the dependency here, at the same seam that swaps the source,
      # and pin the feature explicitly so a missing wasmtime fails at
      # configure time instead of mid-compile. nixpkgs' wasmtime ships the
      # C API + C++ headers in its dev output and libwasmtime via the
      # dev->out propagation of the multiple-outputs hook.
      nix-expr = prev.nix-expr.overrideAttrs (old: {
        buildInputs = (old.buildInputs or []) ++ [componentPkgs.wasmtime];
        mesonFlags = (old.mesonFlags or []) ++ ["-Dwasm=enabled"];
      });
      # See `curlForNixStore` above: libstore owns the file-transfer
      # worker the curl regression stalls, and it is the only component that
      # takes Curl, so the source view is scoped to it instead of `pkgs.curl`.
      nix-store = prev.nix-store.override {curl = curlForNixStore;};
      # The Rust evaluator links into the CLI only -- `src/nix/meson.build`
      # owns the cargo target, and nothing in libexpr/libstore/libfetchers
      # depends on it -- so this is the one component that needs the flag.
      #
      # The meson option defaults to `disabled` rather than `auto` on purpose:
      # nixpkgs' meson hook passes `--auto-features=enabled` (see the wasm note
      # above), which would otherwise switch the Rust build on for every
      # consumer of this package with no cargo in scope.
      nix-cli =
        if !withRustEval
        then prev.nix-cli
        else
          prev.nix-cli.overrideAttrs (old: {
            mesonFlags =
              (old.mesonFlags or [])
              ++ [
                "-Drust-eval=enabled"
                "-Drust-eval-prefix=${nixEvalRs}"
              ];
          });
    });

    # The aggregate `nix` package (daemon + client + libs), the same attribute
    # `nixVersions.nix_2_34` exposes.
    nixEverything = patchedComponents.nix-everything;
  in
    nixEverything.overrideAttrs (old: {
      inherit version;
      passthru =
        (old.passthru or {})
        // {
          # The patched modular component set, for tools that must link the
          # same patched libexpr this daemon-compatible client uses
          # (packages/nix-eval-jobs: the CI evaluator has to parse the
          # same language the client does, underscore digit separators
          # included).
          components = patchedComponents;
          inherit provenance;
        };
      # The aggregate's `doCheck = true` gates the build on `checkInputs`: the
      # five component unit-test runners plus the entire upstream functional
      # suite. Those dominate a cold build of this closure and re-validate
      # nothing per consumer rebuild: the source arrives pre-patched from the
      # fork repo, the series carries its own
      # upstream-style functional test inside the patched tree, and the `smoke`
      # passthru below executes the linked binary. With them on, the cache-push
      # darwin lane (3-core hosted mac) blew its 4 h job budget cold-building
      # this package and froze `cache-ready` (run 28772327218, index#1967).
      doCheck = false;
      # The cross build cannot execute its result (the `smoke` passthru is
      # native-only below), so assert the container format in-build: a
      # mislinked binary (ELF, wrong arch) fails on the linux builder instead
      # of on the first Mac that substitutes it.
      nativeBuildInputs = (old.nativeBuildInputs or []) ++ lib.optional isCross pkgs.file;
      installPhase =
        (old.installPhase or "")
        + ''
          # shell
          install -Dm444 ${provenanceJson} "$out/share/nix/ix-provenance.json"
        ''
        + lib.optionalString isCross ''
          # shell
          format=$(file -bL "$out/bin/nix")
          # file(1) orders arch and kind differently across versions
          # ("Mach-O 64-bit arm64 executable" vs "... executable arm64").
          case $format in
          "Mach-O 64-bit ${machoArch} executable"* | "Mach-O 64-bit executable ${machoArch}"*) ;;
          *)
            echo "expected a Mach-O 64-bit ${machoArch} executable, got: $format" >&2
            exit 1
            ;;
          esac
        '';
      meta =
        (old.meta or {})
        // {
          description = "NixOS/nix ${upstreamVersion} from the index jj view (h${shortHash})";
          mainProgram = "nix";
        };
    });

  package = patchedNix;

  # Test 2414 calls wakeup, perform, then poll. Curl 8.21.0 consumed the public
  # wakeup during perform and left poll asleep until the idle timeout.
  curlMultiWakeup = curlForNixStore.overrideAttrs (_: {
    doCheck = true;
    checkTarget = "test";
    checkFlags = ["TFLAGS=2414"];
  });

  # The override's real risk is that the whole modular C++ tree still links and
  # the installed binary and provenance file agree with the eval-time identity.
  # `--version` exits without touching a store or daemon, so it is safe here.
  smoke =
    pkgs.runCommand "nix-ix-smoke"
    {
      nativeBuildInputs = [
        package
        pkgs.jq
      ];
      strictDeps = true;
    }
    ''
      expected=${lib.escapeShellArg "nix (Nix) ${package.version}"}
      actual=$(nix --version)
      if [[ "$actual" != "$expected" ]]; then
        echo "nix --version disagrees with the package version" >&2
        printf 'expected: %s\nactual:   %s\n' "$expected" "$actual" >&2
        exit 1
      fi

      jq -e \
        --arg version ${lib.escapeShellArg package.version} \
        --arg sourceDigest ${lib.escapeShellArg package.provenance.sourceDigest} \
        --arg sourceStorePath ${lib.escapeShellArg package.provenance.source.storePath} \
        '.schema == 3 and .algorithm == "sha256" and .version == $version and .sourceDigest == $sourceDigest and .source.storePath == $sourceStorePath' \
        ${package}/share/nix/ix-provenance.json >/dev/null

      mkdir -p "$out"
    '';

  focusedFunctionalTest = {
    name,
    testDaemon ? null,
  }: let
    tests = package.components.nix-functional-tests.override (
      lib.optionalAttrs (testDaemon != null) {test-daemon = testDaemon;}
    );
  in
    tests.overrideAttrs (old: {
      mesonCheckFlags = (old.mesonCheckFlags or []) ++ [name];
    });

  autoGcInterrupt = focusedFunctionalTest {name = "gc-auto";};
  # `libfetchers: resolve git refs lazily and refresh the cached HEAD`
  # regression coverage: a cached rev-pinned fetchGit input must evaluate
  # without remote git subprocesses, and the cached HEAD must refresh on a
  # successful network lookup (indexable-inc/index#4028).
  fetchGitHeadCache = focusedFunctionalTest {name = "fetchGit-head-cache";};
  daemonSignal = focusedFunctionalTest {
    name = "daemon-signal";
    testDaemon = package.components.nix-cli;
  };
  buildStatus = focusedFunctionalTest {name = "build-status";};
  # Patch 0024 regression coverage: the upstream relative-paths lock file
  # test now asserts sparse child-lock semantics (stale copied nodes refresh
  # from the child's own flake.lock; in-sync locks stay byte-identical).
  sparseLocks = focusedFunctionalTest {name = "relative-paths-lockfile";};
  # `fix(libstore): don't abort when an output path becomes valid mid-build`
  # regression coverage: a local-overlay store whose LOWER store gains an
  # input-addressed output while the overlay is still building that very
  # derivation must keep the registered path and carry on, not abort the
  # process on `assert(newInfo.ca)`. That is the shape of the ephemeral-upper
  # CI lane (ix#8445), where concurrent jobs publish into the shared durable
  # store the others build against.
  overlayLowerGainsOutput = focusedFunctionalTest {name = "lower-gains-output";};
  # `don't let Darwin discard a fast-exiting builder's log` regression
  # coverage: a builder that writes to stderr and exits at once, while other
  # jobs are starting, must still have its output in the failure message and in
  # `nix log`. On macOS it did not -- XNU flushes a pseudoterminal's output
  # queue about 0.6s after the last slave fd closes, and nix only polls once it
  # has finished starting every runnable child (ENG-11172). This runs on linux
  # too, where it asserts the invariant the darwin fix restores.
  buildLogFastExit = focusedFunctionalTest {name = "build-log-fast-exit";};
  # `libstore: Bit-reproducibly fix darwin Mach-O page hashes after rewriting`
  # regression coverage: after `RewritingSink` mutates bytes the linker had
  # already covered with ad-hoc page hashes, the rewritten binary must still
  # execute, verify under codesign, and keep its `linker-signed` flag rather
  # than being re-signed. The test expects `--check` itself to fail (LC_UUID is
  # still a stale content hash, index#4336) and inspects the `.check` binary.
  machoRewrite = focusedFunctionalTest {name = "macho-rewrite";};
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit bootstrapLock;
        # Execution tests are native-only: the cross package's binary cannot
        # run on the linux build host (its format is asserted in-build
        # instead), and the check catalog collects tests from the native
        # `repoPackages` entry only.
        tests =
          (old.passthru.tests or old.tests or {})
          // lib.optionalAttrs (!isCross) {
            inherit autoGcInterrupt buildLogFastExit buildStatus curlMultiWakeup daemonSignal fetchGitHeadCache machoRewrite overlayLowerGainsOutput smoke sparseLocks;
          };
      }
      // lib.optionalAttrs (updateScriptWriter != null) {
        updateScript = updateScriptWriter updateScriptArgs;
      };
  })
