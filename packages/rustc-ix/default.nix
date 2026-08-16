{
  fetchFromGitHub,
  fetchgit,
  ix,
  lib,
  llvmPackages_21,
  pkgsCross,
  python3,
  rustPlatform,
  stdenv,
  symlinkJoin,
  zlib,
}:
# The ix rustc fork (indexable-inc/rustc, branch `ix`), built from source with
# the fork's own `./x` bootstrap and assembled into a toolchain directory
# shaped like the rust-overlay aggregates cargo-unit already consumes:
# `bin/rustc`, `bin/rustdoc`, `bin/cargo`, `lib/rustlib/<triple>/lib`, and
# `lib/rustlib/src/rust` (rust-src). It slots into
# `cargoUnit.buildWorkspace { rustToolchain = ...; }` unchanged, and
# `rust.toolchainId` (the store-path basename baked into every unit hash)
# distinguishes it from the default toolchain automatically.
#
# Why from source at all: the fork carries `-Zdump-test-names` (fork PR #1),
# which makes `rustc --test` emit the collected #[test]/#[bench] descriptors
# as JSON and stop before codegen and linking. That is the missing upstream
# capability (rust-lang/rust#50297) that lets cargo-unit's per-#[test]
# discovery run without compiling and linking every test binary inside the
# eval-blocking manifest IFD.
#
# Why not nixpkgs' rustc machinery with `src` overridden (the house-preferred
# route): nixpkgs builds rustc from RELEASE tarballs, bootstrapping each
# version with its own previous nixpkgs rustc. The pinned nixpkgs carries
# rustc 1.97.0 stable; the fork is 1.99.0-nightly master, whose `src/stage0`
# requires the 2026-07-13 beta (1.98) as stage0. rustc only supports
# bootstrapping from the current cycle's beta (cfg(bootstrap) gates are keyed
# to exactly one version back), so nixpkgs' 1.97 bootstrap cannot compile this
# tree, and nixpkgs has no 1.98 to chain through. Driving the fork's own
# bootstrap hermetically costs less than forking nixpkgs' version chain:
#
#   * stage0: the exact beta `src/stage0` records, taken from the pinned
#     rust-overlay (a nix-built toolchain, so no binary patching and no
#     network), never downloaded by x.py.
#   * LLVM: the pinned nixpkgs' llvmPackages_21 via `llvm-config` (the fork
#     requires LLVM >= 21; see src/bootstrap .. llvm.rs check_llvm_version),
#     so no `download-ci-llvm` (whose artifact URL derives from git history a
#     GitHub tarball does not carry) and no 2 GiB llvm-project submodule.
#   * crates: all three lockfiles (Cargo.lock, library/Cargo.lock,
#     src/bootstrap/Cargo.lock) are registry-only and vendored through
#     `rustPlatform.importCargoLock`, which pins every crate by the lockfile
#     checksum, so the vendor tree needs no fixed-output hash to maintain.
#   * submodules: only `library/backtrace` is needed for a compiler+std build
#     (std textually includes its sources); it is pinned in pins.json at the
#     gitlink rev the fork records. llvm-project/gcc/enzyme/doc submodules are
#     never touched (`build.submodules = false`).
#
# The build is a stage1 compiler + rustdoc + std for the three targets the
# workspace's cargo-unit lanes actually plan on x86_64-linux (enumerated
# against nix/packages/workspace-{bins,trees,sdks,rust-ci}.nix before the
# default-toolchain flip):
#
#   * x86_64-unknown-linux-gnu — the host spine: every build/test/clippy-dep
#     graph, plus nextest's `--rust-libdir`.
#   * x86_64-unknown-linux-musl — the static guest fleet binaries and the
#     public CLI's linux-x86_64 lane. Needs a musl cross cc and a musl root
#     (crt objects + libc.a copied into the target's `self-contained` dir),
#     both from nixpkgs' pkgsCross.musl64; the `unwind` crate needs a static
#     libunwind, built in-tree by bootstrap's cc-only Libunwind step from a
#     sparse checkout of llvm-project/libunwind at the fork's recorded
#     gitlink (pins.json) — the one llvm-project piece this build reads.
#   * wasm32-unknown-unknown — the term-predict/room-sdk/audio-dsp/sdk wasm
#     lanes. Pure-Rust std (no C with optimized-compiler-builtins off); its
#     default linker is `rust-lld` from the sysroot, which this build ships
#     by installing the external LLVM 21 `lld` binary at
#     `lib/rustlib/<host>/bin/rust-lld` (same LLVM major the compiler links,
#     and rust-lld upstream is exactly in-tree lld) instead of building lld
#     from an llvm-project source tree it does not have.
#
# aarch64-apple-darwin is deliberately NOT here: the two lanes that
# cross-build it from Linux (the public CLI's darwin-arm64 half and the
# darwin cross tools) pin the upstream toolchain explicitly in
# nix/packages/workspace-bins.nix — building darwin std would drag the Apple
# SDK into this derivation for two dist lanes that gain nothing from rmeta
# cutoff. A stage1 build, not a full `x dist`: a stage2 rebuild would compile
# the same source a second time with the compiler it just built, which buys
# reproducibility properties no consumer here needs.
#
# What the aggregate deliberately omits, relative to the rust-overlay default
# toolchain: clippy-driver (per-unit clippy uses its own pinned toolchain,
# `policy.clippy.package.toolchain`, never the workspace's — verified in
# index/lib/rust/cargo-unit.nix's clippyUnits import), rustfmt (the only
# format gate, treefmt's rustfmt, builds its own rust-bin component),
# rust-analyzer, and `lib/rustlib/<triple>/bin/llvm-{cov,profdata}` (coverage
# callers must pass explicit tool paths, which `makeCoverageReport` supports;
# no ix lane exercises coverage).
# `bin/cargo` is the stage0 beta's cargo, copied in: cargo is
# version-agnostic here (the planner IFD already sets RUSTC_BOOTSTRAP=1 for
# `--unit-graph`), and copying the one binary keeps the stage0 toolchain out
# of the runtime closure.
#
# Pin bumps (e.g. picking up the fork's rmeta-stability flags when that PR
# merges) are a pins.json edit: rev + hash for the fork, and, when the fork's
# `src/stage0` or `library/backtrace` gitlink move, the stage0 date below and
# the backtrace pin. Nothing else in this file encodes the fork revision.
let
  pins = ix.pins.loadPins ./pins.json;
  srcPin = pins.rustc-ix-src;
  backtracePin = pins.backtrace-src;

  src = fetchFromGitHub {
    inherit (srcPin) owner repo rev hash;
  };

  backtraceSrc = fetchFromGitHub {
    inherit (backtracePin) owner repo rev hash;
  };

  libunwindPin = pins.llvm-project-libunwind-src;
  # Only the two llvm-project subtrees the musl std build reads, at the
  # gitlink the fork records: libunwind (bootstrap's Libunwind step compiles
  # its C/C++ sources directly with the target cc — no cmake, no LLVM build)
  # and compiler-rt/lib/builtins (the CrtBeginEnd step compiles crtbegin.c /
  # crtend.c for musl's self-contained linking the same way). A sparse
  # checkout because the full llvm-project tree is ~2 GiB for two
  # directories this build reads.
  llvmProjectLibunwind = fetchgit {
    url = "https://github.com/${libunwindPin.owner}/${libunwindPin.repo}";
    inherit (libunwindPin) rev hash;
    sparseCheckout = [
      "libunwind"
      "compiler-rt/lib/builtins"
    ];
  };

  # The musl cross toolchain for the x86_64-unknown-linux-musl std: the
  # cc/ar wrappers compile bootstrap's in-tree libunwind and any C a std
  # dependency carries, and the musl package is the `musl-root` bootstrap
  # copies crt objects and libc.a out of (into the target's self-contained
  # dir, which is what rustc's default `crt-static` musl links against).
  muslCc = pkgsCross.musl64.stdenv.cc;
  musl = pkgsCross.musl64.musl;
  muslTriple = "x86_64-unknown-linux-musl";
  wasmTriple = "wasm32-unknown-unknown";

  # The exact stage0 the fork's `src/stage0` records (compiler_version=beta,
  # compiler_date=2026-07-13). rustc master only builds with the current
  # cycle's beta, so this moves together with the fork pin, not with the
  # repo's own rust-toolchain.toml. rustfmt is included only to satisfy
  # bootstrap's unconditional `initial_rustfmt` resolution at config parse
  # (`build.rustfmt` unset means "download one", which a sandbox cannot);
  # nothing in this build ever formats, so its version is irrelevant.
  stage0 = ix.languages.rust.toolchain ix.pkgs {
    channel = "beta";
    version = "2026-07-13";
    components = [
      "cargo"
      "rust-std"
      "rustc"
      "rustfmt"
    ];
  };

  # All three cargo workspaces the stage1 build touches: bootstrap itself,
  # the compiler workspace (root), and std. importCargoLock pins each crate
  # by its lockfile checksum; a same-crate collision between the three vendor
  # trees is byte-identical content, so the symlinkJoin merge is safe.
  vendor = symlinkJoin {
    name = "rustc-ix-vendor";
    paths = map (lockFile: rustPlatform.importCargoLock {inherit lockFile;}) [
      "${src}/Cargo.lock"
      "${src}/library/Cargo.lock"
      "${src}/src/bootstrap/Cargo.lock"
    ];
  };

  inherit (llvmPackages_21) llvm;

  hostTriple = stdenv.hostPlatform.rust.rustcTarget;
in
  stdenv.mkDerivation {
    pname = "rustc-ix";
    # The store-path basename is the cargo-unit toolchainId, so carry the fork
    # rev in the version: two fork pins can never alias to one unit hash.
    version = "${srcPin.version}-nightly-ix-${builtins.substring 0 12 srcPin.rev}";

    inherit src;

    # muslCc is deliberately NOT in nativeBuildInputs: a second cc-wrapper's
    # setup hooks contaminate the host wrapper's environment (the bootstrap's
    # own build scripts then link with the musl ld against gnu libstd and die
    # on `stat64`). Everything musl-targeted reaches the cross tools through
    # the absolute paths in bootstrap.toml instead.
    nativeBuildInputs = [
      python3
      stage0
    ];
    buildInputs = [
      llvm
      zlib
    ];

    # A rustc build wants a wide machine; the fleet builders advertise this.
    requiredSystemFeatures = ["big-parallel"];
    enableParallelBuilding = true;

    # rustc's own suite runs through `x.py test`, far beyond this build's
    # budget; the cargo-unit-fork-discovery check exercises the toolchain.
    doCheck = false;

    postPatch = ''
      # shell
      # The two submodule subtrees this build reads: std textually includes
      # backtrace's sources, and the musl target's in-tree libunwind builds
      # from llvm-project/libunwind (bootstrap's require_submodule only
      # checks the directory is populated when submodule management is off).
      # The GitHub tarball has empty gitlink dirs.
      rmdir library/backtrace
      cp -r ${backtraceSrc} library/backtrace
      rmdir src/llvm-project
      cp -r ${llvmProjectLibunwind} src/llvm-project
      chmod -R u+w src/llvm-project
    '';

    configurePhase = ''
      # shell
      runHook preConfigure

      export HOME="$TMPDIR/home"
      export CARGO_HOME="$TMPDIR/cargo"
      mkdir -p "$HOME" "$CARGO_HOME"

      # bootstrap.py's vendoring check wants <src>/vendor and <src>/.cargo to
      # exist; every cargo invocation runs from the source root, so this one
      # config covers the bootstrap, compiler, and library workspaces.
      ln -s ${vendor} vendor
      mkdir -p .cargo
      cat > .cargo/config.toml <<EOF
      [source.crates-io]
      replace-with = "vendored-sources"
      [source.vendored-sources]
      directory = "$PWD/vendor"
      EOF

      cat > bootstrap.toml <<EOF
      change-id = "ignore"

      [build]
      build = "${hostTriple}"
      host = ["${hostTriple}"]
      target = ["${hostTriple}", "${muslTriple}", "${wasmTriple}"]
      rustc = "${stage0}/bin/rustc"
      cargo = "${stage0}/bin/cargo"
      rustfmt = "${stage0}/bin/rustfmt"
      python = "${python3.interpreter}"
      vendor = true
      locked-deps = true
      submodules = false
      docs = false
      extended = false
      # Optimized builtins come from llvm-project/compiler-rt, a submodule
      # this build never checks out; the pure-Rust fallbacks are what every
      # non-dist build uses.
      optimized-compiler-builtins = false

      [rust]
      channel = "nightly"
      debuginfo-level = 0
      deny-warnings = false
      llvm-tools = false
      lld = false
      download-rustc = false

      [llvm]
      download-ci-llvm = false
      link-shared = true

      [target.${hostTriple}]
      llvm-config = "${llvm.dev}/bin/llvm-config"
      cc = "$CC"
      cxx = "$CXX"
      linker = "$CC"
      ar = "$AR"
      ranlib = "$RANLIB"

      # Static musl std: cc/ar drive the in-tree libunwind compile and any C
      # a std dep carries; musl-root is where bootstrap takes the crt objects
      # and libc.a it stages into lib/rustlib/${muslTriple}/lib/self-contained.
      # llvm-libunwind scoped to this target so the gnu target keeps its
      # normal gcc_s unwinder.
      [target.${muslTriple}]
      cc = "${muslCc}/bin/${muslTriple}-cc"
      cxx = "${muslCc}/bin/${muslTriple}-c++"
      linker = "${muslCc}/bin/${muslTriple}-cc"
      ar = "${muslCc.bintools}/bin/${muslTriple}-ar"
      ranlib = "${muslCc.bintools}/bin/${muslTriple}-ranlib"
      musl-root = "${musl.out}"
      llvm-libunwind = "in-tree"

      # Pure-Rust std (no C at all with optimized-compiler-builtins off), but
      # bootstrap still resolves a target cc at config time; the unwrapped
      # clang can target wasm and never injects host hardening flags the wasm
      # backend rejects (the same reasoning as wasmCcEnv in
      # lib/workspace-cargo-unit.nix).
      [target.${wasmTriple}]
      cc = "${llvmPackages_21.clang-unwrapped}/bin/clang"
      linker = "${llvmPackages_21.clang-unwrapped}/bin/clang"
      EOF

      runHook postConfigure
    '';

    buildPhase = ''
      # shell
      runHook preBuild

      python3 x.py build -j "$NIX_BUILD_CORES" --stage 1 library src/tools/rustdoc

      runHook postBuild
    '';

    installPhase = ''
      # shell
      runHook preInstall

      stage1="build/${hostTriple}/stage1"

      mkdir -p "$out"
      cp -a "$stage1/bin" "$out/bin"
      cp -a "$stage1/lib" "$out/lib"

      # bootstrap wires the stage sysroot's rust-src (lib/rustlib/src/rust)
      # as a symlink into the build tree, and may leave other build-tree
      # links; every one would dangle in the store (or, worse, reroute the
      # rust-src copy below back into the sandbox). Drop them and install a
      # real rust-src copy instead.
      find "$out" -type l | while read -r link; do
        case "$(readlink "$link")" in
          /build/*) rm -v "$link" ;;
        esac
      done
      rm -rf "$out/lib/rustlib/src"

      # rust-lld, for the wasm32 lanes: rustc resolves the wasm target's
      # default linker at lib/rustlib/<host>/bin/rust-lld inside its own
      # sysroot. Upstream's rust-lld is exactly in-tree lld; installing the
      # external LLVM 21 lld binary (the same major the compiler links)
      # gives the identical linker without building lld from an llvm-project
      # source tree this derivation does not have.
      install -D -m755 "${llvmPackages_21.lld}/bin/lld" \
        "$out/lib/rustlib/${hostTriple}/bin/rust-lld"

      # Stage1 tools land beside the sysroot, not in it; rustdoc links
      # librustc_driver via an ORIGIN-relative rpath, so placing it in
      # "$out"/bin resolves against the stage1 libs copied above.
      if [ ! -e "$out/bin/rustdoc" ]; then
        rustdoc_bin="$(find build/${hostTriple} -maxdepth 2 -name rustdoc -type f -path '*stage1*' | head -n1)"
        if [ -z "$rustdoc_bin" ]; then
          echo >&2 "error: stage1 rustdoc not found in build tree"
          exit 1
        fi
        cp "$rustdoc_bin" "$out/bin/rustdoc"
      fi

      # cargo: the stage0 beta's binary, copied (not symlinked) so the stage0
      # aggregate is not retained; the ELF's own store references (glibc,
      # patched rpaths) survive the copy and are all it needs.
      cp -L ${stage0}/bin/cargo "$out/bin/cargo"

      # rust-src, at the path cargo-unit remaps out of every unit
      # (lib/rustlib/src/rust). The component layout is the source tree's
      # library/ plus the workspace files rust-analyzer and -Zbuild-std
      # expect.
      mkdir -p "$out/lib/rustlib/src/rust"
      cp -a library "$out/lib/rustlib/src/rust/library"
      cp Cargo.toml Cargo.lock "$out/lib/rustlib/src/rust/"

      runHook postInstall
    '';

    # rlibs are ar archives carrying rustc metadata; a blanket strip pass has
    # nothing to gain (debuginfo-level=0 already) and archives to corrupt.
    dontStrip = true;

    passthru = {
      inherit src stage0;
      forkRev = srcPin.rev;
    };

    meta = {
      description = "ix rustc fork (indexable-inc/rustc) as a cargo-unit-shaped Rust toolchain, carrying -Zdump-test-names";
      homepage = "https://github.com/indexable-inc/rustc";
      license = [lib.licenses.mit lib.licenses.asl20];
      platforms = ["x86_64-linux"];
    };
  }
