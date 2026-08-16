# GHC as a Linux-hosted cross compiler targeting Darwin (RFC 0009 lane, #3606).
#
# nixpkgs' pkgsCross road to a Darwin target dies bootstrapping the Apple SDK
# from Apple-only binaries (#3584, nixpkgs#405893). This builds the compiler
# the way upstream's wasm and mingw cross lanes do instead: GHC's own hadrian
# cross build (`./configure --target=<triple>`) with the repo's apple-sdk
# standalone clang lane as the target toolchain and llvm binutils as the
# Mach-O tool set. The boot compiler, alex/happy and the pure hadrian
# bootstrap all come from the pinned nixpkgs, so the compiler version always
# matches `haskellPackages.ghc`. The source view carries the aarch64 host versus
# target fixes, Cabal's `osx` OS spelling, and the cross-compatible libffi bundle
# as ordinary commits.
#
# The stage1 compiler this produces runs on the Linux build host and emits
# Mach-O arm64; ld64.lld ad-hoc signs output, so binaries run on Apple
# Silicon unmodified. Template Haskell splice *execution* is impossible in
# this lane (no Darwin iserv on Linux); the nom closure needs none (#3606).
{ghcSrc}: {
  autoconf,
  automake,
  haskellPackages,
  lib,
  lld,
  llvmPackages,
  # `pkgs.path`: the pinned nixpkgs source tree, for its pure hadrian
  # bootstrap helper.
  nixpkgsPath,
  perl,
  python3,
  ripgrep,
  runCommand,
  stdenv,
  target,
  # ix.appleSdkToolchain instance for `target`; supplies the standalone
  # clang/clang++ drivers that compile C for the target and link Mach-O
  # through ld64.lld.
  toolchain,
}: let
  bootGhc = haskellPackages.ghc;

  # GHC's build system, built from the same source tree with the boot
  # compiler; nixpkgs' helper vendors the dependency plan, so no network.
  hadrian =
    import (nixpkgsPath + "/pkgs/development/tools/haskell/hadrian/make-hadrian.nix") {
      bootPkgs = haskellPackages;
      inherit lib;
    } {
      inherit ghcSrc;
      ghcVersion = bootGhc.version;
    };

  bintools = llvmPackages.bintools-unwrapped;
  llvmTool = name: lib.getExe' bintools name;
in
  assert lib.assertMsg (target == "aarch64-apple-darwin")
  "ix.crossGhc: only aarch64-apple-darwin is wired up (the NCG patch set is aarch64-specific)";
  assert lib.assertMsg (bootGhc.version == "9.10.3")
  "ix.crossGhc: nixpkgs moved GHC from 9.10.3 to ${bootGhc.version}; update index/views/ghc-cross and its build metadata together";
    stdenv.mkDerivation {
      pname = "ghc-cross-${target}";
      inherit (bootGhc) version;
      src = ghcSrc;

      # A cross GHC cannot run its own testsuite on the build host; the
      # cross-darwin-ghc-smoke check exercises the result instead.
      doCheck = false;

      postPatch = ''
        # shell
        patchShebangs --build .
      '';

      strictDeps = true;
      nativeBuildInputs = [
        bootGhc
        hadrian
        haskellPackages.alex
        haskellPackages.happy
        python3
        perl
        autoconf
        automake
        # Puts dsymutil on PATH: clang runs it after linking whenever -g is in
        # effect (configure's default CFLAGS), and LLVM's reads/writes Mach-O
        # fine from Linux.
        bintools
      ];

      configureFlags = [
        "--target=${target}"
        # Target toolchain: the apple-sdk standalone clang lane + llvm
        # binutils. All Mach-O-capable, all run on the Linux build host.
        "CC=${toolchain.standaloneCc}"
        "CXX=${toolchain.standaloneCxx}"
        "LD=${lib.getExe' lld "ld64.lld"}"
        "AR=${llvmTool "llvm-ar"}"
        "RANLIB=${llvmTool "llvm-ranlib"}"
        "NM=${llvmTool "llvm-nm"}"
        "OBJDUMP=${llvmTool "llvm-objdump"}"
        "OTOOL=${llvmTool "llvm-otool"}"
        "INSTALL_NAME_TOOL=${llvmTool "llvm-install-name-tool"}"
        # ld64.lld implements no `ld -r`; an explicitly empty MergeObjsCmd is
        # GHC's sanctioned "do without object merging" (ar-join) path.
        "MergeObjsCmd="
        "GHC=${lib.getExe' bootGhc "ghc"}"
      ];

      preConfigure = ''
        # shell
        # ld64.lld's version banner satisfies libtool's GNU-ld probe, which
        # would make the bundled libffi link its dylib with ELF flag syntax
        # (--version-script/-soname) that ld64.lld rejects. A config.site
        # reaches every sub-configure hadrian runs.
        echo 'lt_cv_prog_gnu_ld=no' > "$TMPDIR/config.site"
        export CONFIG_SITE="$TMPDIR/config.site"
      '';

      buildPhase = ''
        # shell
        runHook preBuild
        # quick: -O2 stage1 compiler, -O1 target libraries -- the validated
        # pipeline (#3606); release only buys longer CI for a log renderer's
        # closure. native_bignum drops the gmp dependency entirely.
        hadrian --flavour=quick+native_bignum --docs=none -j"$NIX_BUILD_CORES" binary-dist-dir
        runHook postBuild
      '';

      installPhase = ''
        # shell
        runHook preInstall
        # The bindist re-runs its own configure to write final settings; give
        # it the same target toolchain so the recorded tool paths are the
        # store paths above.
        pushd _build/bindist/ghc-*
        # configureFlags entries are all space-free (single word or VAR=path), so
        # the flat-string expansion splits correctly. A cross bindist infers
        # build/host from its *content* platform (the target); --host names
        # where the compiler actually runs, or configure tries to execute a
        # Mach-O test binary on Linux.
        ./configure $configureFlags --host=${stdenv.buildPlatform.config} --prefix="$out"
        make install
        popd
        runHook postInstall
      '';

      # Outputs are Linux ELF (the compiler) plus Mach-O target libraries the
      # Linux strip cannot parse; the compiler is already -O2 and never
      # shipped to users.
      dontStrip = true;

      passthru = {
        inherit target toolchain;
        # `<triple>-ghc` etc.; consumers build tool paths with this.
        targetPrefix = "${target}-";
        tests.rtsCabalOsxConfigure =
          runCommand "ghc-rts-cabal-osx-configure" {
            nativeBuildInputs = [
              ripgrep
              stdenv.cc
            ];
          } ''
            mkdir build
            cd build
            CC=cc ${ghcSrc}/rts/configure \
              --build=x86_64-unknown-linux \
              --host=aarch64-unknown-osx

            cat > expected-host-os <<'EOF'
            #define darwin_HOST_OS  1
            #define HOST_OS  "darwin"
            EOF
            rg '^#define (darwin_HOST_OS|HOST_OS)' ghcplatform.h.top > actual-host-os
            diff -u expected-host-os actual-host-os
            touch "$out"
          '';
      };

      meta = {
        description = "GHC ${bootGhc.version} cross compiler: x86_64-linux -> ${target}";
        homepage = "https://www.haskell.org/ghc/";
        license = lib.licenses.bsd3;
        platforms = ["x86_64-linux"];
      };
    }
