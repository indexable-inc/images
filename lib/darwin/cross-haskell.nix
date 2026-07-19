# Build a Haskell package and its library closure with a Linux-hosted cross
# GHC (ix.crossGhc), outside nixpkgs' haskellPackages machinery (#3606).
#
# nixpkgs' generic-builder keys its cross behaviour on the stdenv's
# build/host platform split, which only exists under pkgsCross -- and the
# pkgsCross road to Darwin dies in the Apple SDK bootstrap (#3584). This is
# the same recipe reduced to its essentials, with the platform split carried
# by the compiler instead of the stdenv:
#
#   - `Setup` is compiled with the *native* boot GHC (it must run on the
#     build host); the package itself is configured `--with-compiler=` the
#     cross GHC. Cabal derives the host platform from the compiler, so
#     autoconf-style Setups (streamly-core) pass the right --host along.
#   - Metadata is not duplicated: sources, versions and the dependency graph
#     come from the pinned nixpkgs haskellPackages derivations (hackage2nix's
#     getCabalDeps), so a nixpkgs bump moves this closure automatically.
#   - GHC boot libraries appear as `null` in getCabalDeps and ship inside the
#     cross GHC's global package db already.
#
# Template Haskell splice execution is impossible in this lane; the audit in
# #3606 verified the nom closure never runs a splice (quotes-only TH).
{
  # ix.crossGhc instance; carries `target` + `targetPrefix` passthru.
  crossGhc,
  haskellPackages,
  lib,
  llvmPackages,
  stdenv,
}: let
  hp = haskellPackages;
  nativeGhc = hp.ghc;
  bintools = llvmPackages.bintools-unwrapped;

  targetTool = name: "${crossGhc}/bin/${crossGhc.targetPrefix}${name}";

  # Direct Haskell deps from hackage2nix metadata; boot libs are null. Tool
  # deps (alex/happy) are host programs, provided globally below. Executable
  # deps ride along because configure is whole-package: a package whose
  # executable needs more than its library (nix-derivation's
  # pretty-derivation wants pretty-show) still has to resolve it.
  libDepsOf = drv:
    lib.unique (builtins.filter (d: d != null && (d.isHaskellLibrary or false)) (
      (drv.getCabalDeps.libraryHaskellDepends or [])
      ++ (drv.getCabalDeps.executableHaskellDepends or [])
      ++ (drv.getCabalDeps.setupHaskellDepends or [])
    ));

  buildOne = {
    hsDrv,
    # Direct dependencies as *cross-built* derivations (each carries
    # `passthru.crossHaskellClosure`).
    deps,
    patches ? [],
    extraConfigureFlags ? [],
    # Configure the whole package, not `lib:<pname>`: per-component setup
    # cannot see internal sub-libraries (attoparsec:attoparsec-internal),
    # and tests/benchmarks stay disabled by default so their dependencies
    # never enter the picture.
    postInstall ? "",
    extraNativeBuildInputs ? [],
  }: let
    closure = lib.unique (deps ++ lib.concatMap (d: d.passthru.crossHaskellClosure) deps);
  in
    stdenv.mkDerivation {
      pname = "${hsDrv.pname}-${crossGhc.target}";
      inherit (hsDrv) version src;
      inherit patches postInstall;

      strictDeps = true;
      nativeBuildInputs =
        [
          crossGhc
          nativeGhc
          hp.alex
          hp.happy
          hp.jailbreak-cabal
          # llvm-ar & co. for Cabal's own archive handling of Mach-O members.
          bintools
        ]
        ++ extraNativeBuildInputs;

      # Space-free store paths, consumed by the db-assembly loop below.
      crossHaskellDeps = map (d: d.outPath) closure;

      configurePhase = ''
        # shell
        runHook preConfigure

        # One combined package db of the transitive dependency closure; conf
        # files carry absolute store paths, so copy + recache is sufficient.
        pkgdb="$TMPDIR/cross-pkgdb"
        mkdir -p "$pkgdb"
        for dep in $crossHaskellDeps; do
          if [ -d "$dep/lib/package.conf.d" ]; then
            cp "$dep"/lib/package.conf.d/*.conf "$pkgdb/"
          fi
        done
        ${targetTool "ghc-pkg"} --package-db="$pkgdb" recache

        # Setup runs on the build host: compile it with the native GHC. Most
        # sdists ship no Setup.hs; synthesize the one their build-type means.
        if [ ! -e Setup.hs ] && [ ! -e Setup.lhs ]; then
          if grep -qiE '^build-type:[[:space:]]*Configure' ./*.cabal; then
            printf 'import Distribution.Simple\nmain = defaultMainWithHooks autoconfUserHooks\n' > Setup.hs
          else
            printf 'import Distribution.Simple\nmain = defaultMain\n' > Setup.hs
          fi
        fi
        ${lib.getExe' nativeGhc "ghc"} --make Setup -o Setup -v0

        # --disable-library-for-ghci: the GHCi object (HS<pkg>.o) is produced
        # by an `ld -r` merge, which resolves to the host's ELF lld and dies
        # on Mach-O members ("unknown file type"); no Mach-O `ld -r` exists in
        # this lane (the compiler itself runs with MergeObjsCmd="", the
        # ar-join path) and nothing can load GHCi objects here anyway -- no
        # Darwin iserv runs on the Linux host.
        # Strip version bounds first (nixpkgs' jailbreak mechanism): sdist
        # tarballs carry the original bounds, while the relaxations nixpkgs
        # relies on live in hackage cabal-file *revisions* (hackage2nix's
        # editedCabalFile), which a plain unpack never sees. The version set
        # is exactly the pinned nixpkgs one, already proven coherent by the
        # native build, so bounds carry no information here.
        for cabalFile in ./*.cabal; do
          jailbreak-cabal "$cabalFile"
        done

        ./Setup configure \
          --with-compiler=${targetTool "ghc"} \
          --with-hc-pkg=${targetTool "ghc-pkg"} \
          --with-hsc2hs=${targetTool "hsc2hs"} \
          --hsc2hs-option=--cross-compile \
          --with-ar=${lib.getExe' bintools "llvm-ar"} \
          --with-strip=${lib.getExe' bintools "llvm-strip"} \
          --package-db="$pkgdb" \
          --prefix="$out" \
          --libdir='$prefix/lib' \
          --disable-shared \
          --disable-library-for-ghci \
          --disable-executable-stripping \
          --disable-library-stripping \
          --ghc-options=-j"$NIX_BUILD_CORES" \
          ${lib.concatStringsSep " " extraConfigureFlags}

        runHook postConfigure
      '';

      buildPhase = ''
        # shell
        runHook preBuild
        ./Setup build
        runHook postBuild
      '';

      installPhase = ''
        # shell
        runHook preInstall
        ./Setup copy
        # Register libraries so dependents can assemble their dbs; packages
        # without a library component just skip this.
        if ./Setup register --gen-pkg-config=cross-pkg.conf 2>/dev/null; then
          mkdir -p "$out/lib/package.conf.d"
          if [ -d cross-pkg.conf ]; then
            # Multi-library packages (internal sub-libraries: attoparsec,
            # vector) yield a directory with one conf per library, not all
            # of them suffixed .conf -- normalize, ghc-pkg recache only
            # reads *.conf.
            for conf in cross-pkg.conf/*; do
              base=$(basename "$conf")
              cp "$conf" "$out/lib/package.conf.d/''${base%.conf}.conf"
            done
          elif [ -f cross-pkg.conf ]; then
            cp cross-pkg.conf "$out/lib/package.conf.d/${hsDrv.pname}-${hsDrv.version}.conf"
          fi
        fi
        runHook postInstall
      '';

      # Outputs are Mach-O objects/archives (and for nom a Mach-O arm64
      # executable); the Linux fixup strip cannot parse them.
      dontStrip = true;

      passthru = {
        crossHaskellClosure = closure;
        inherit hsDrv;
      };

      # Meta rides along unchanged: the Darwin package alias evaluates this
      # derivation on aarch64-darwin, so pinning platforms to the Linux build
      # host would break the alias (same reasoning as btop's cross build).
      meta = hsDrv.meta or {};
    };
in {
  # Cross-build `root` (a nixpkgs haskellPackages-style derivation) and its
  # closure. `patchesFor.<pname>` appends patches to that closure member;
  # `postInstall` and the executable toggle apply to the root package.
  build = {
    root,
    patchesFor ? {},
    # Extra `Setup configure` flags per closure package, keyed by pname.
    configureFlagsFor ? {},
    postInstall ? "",
    extraNativeBuildInputs ? [],
  }: let
    rootDeps = lib.unique (builtins.filter (d: d != null && (d.isHaskellLibrary or false)) (
      (root.getCabalDeps.libraryHaskellDepends or [])
      ++ (root.getCabalDeps.executableHaskellDepends or [])
      ++ (root.getCabalDeps.setupHaskellDepends or [])
    ));
    closureItems = builtins.genericClosure {
      startSet =
        map (d: {
          key = d.pname;
          drv = d;
        })
        rootDeps;
      operator = item:
        map (d: {
          key = d.pname;
          drv = d;
        }) (libDepsOf item.drv);
    };
    crossSet = lib.fix (self:
      lib.mergeAttrsList (map (item: {
          ${item.key} = buildOne {
            hsDrv = item.drv;
            deps = map (d: self.${d.pname}) (libDepsOf item.drv);
            patches = patchesFor.${item.key} or [];
            extraConfigureFlags = configureFlagsFor.${item.key} or [];
          };
        })
        closureItems));
  in
    buildOne {
      hsDrv = root;
      deps = map (d: crossSet.${d.pname}) rootDeps;
      patches = patchesFor.${root.pname} or [];
      extraConfigureFlags = configureFlagsFor.${root.pname} or [];
      inherit extraNativeBuildInputs postInstall;
    };
}
