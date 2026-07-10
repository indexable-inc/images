_:
/**
Build a Zig package from a `build.zig` / `build.zig.zon` project.

Zig projects declare package metadata and remote dependencies in
`build.zig.zon`. The dependency entries are content-addressed by Zig hashes;
nixpkgs still needs one fixed-output hash for the realized Zig package cache,
so projects with remote dependencies pass `zigDepsHash` after updating
`build.zig.zon`.

Named Zig test steps are exposed as separate derivations under
`passthru.tests`. That lets flake checks schedule independent test steps in
parallel instead of hiding the whole test surface inside the package build.

Arguments:
- `pname`, `version`: derivation identity.
- `src`: project root containing `build.zig` and `build.zig.zon`.
- `zig`: Zig compiler package. Defaults to `pkgs.zig`.
- `zigDepsHash`: Nix hash for the Zig dependency cache, required when the
  project has remote `build.zig.zon` dependencies.
- `zigBuildFlags`, `zigInstallFlags`: extra flags for `zig build` phases.
- `testSteps`: attrset of test names to Zig build step names.
- `meta`: standard derivation meta.
*/
pkgs:
# astlog-ignore: no-at-pattern-shortcut
{
  pname,
  version ? "0.0.0",
  src,
  zig ? pkgs.zig,
  zigDepsHash ? null,
  zigFetchAll ? false,
  zigBuildFlags ? [],
  zigInstallFlags ? [],
  zigDefaultFlags ? [
    "-Dcpu=baseline"
    "--release=safe"
  ],
  nativeBuildInputs ? [],
  buildInputs ? [],
  env ? {},
  testSteps ? {
    default = "test";
  },
  passthru ? {},
  meta ? {},
  ...
} @ rawArgs: let
  inherit (pkgs) lib;

  zigDepsFetcher = zig.fetchDeps or null;

  zigDeps =
    rawArgs.zigDeps or (
      if zigDepsHash == null
      then null
      else if zigDepsFetcher == null
      then
        throw ''
          buildZigPackage: ${zig.name or "the selected Zig compiler"} does not expose fetchDeps.
          Pass a Zig compiler with fetchDeps support or override zigDeps directly.
        ''
      else
        zigDepsFetcher {
          inherit pname version src;
          fetchAll = zigFetchAll;
          hash = zigDepsHash;
        }
    );

  commonAttrs = builtins.removeAttrs rawArgs [
    "env"
    "meta"
    "nativeBuildInputs"
    "passthru"
    "testSteps"
    "zig"
    "zigBuildFlags"
    "zigDeps"
    "zigDepsHash"
    "zigFetchAll"
    "zigDefaultFlags"
    "zigInstallFlags"
  ];

  zigCacheScript =
    ''
      export ZIG_GLOBAL_CACHE_DIR="$TMPDIR/zig-cache"
      mkdir -p "$ZIG_GLOBAL_CACHE_DIR/p" "$ZIG_GLOBAL_CACHE_DIR/tmp"
    ''
    + lib.optionalString (zigDeps != null) ''
      cp -R ${zigDeps}/. "$ZIG_GLOBAL_CACHE_DIR/p/"
      chmod -R u+w "$ZIG_GLOBAL_CACHE_DIR/p"
    '';

  testFor = name: step:
    pkgs.runCommand "${pname}-zig-${name}"
    (
      {
        inherit src buildInputs;
        strictDeps = true;
        nativeBuildInputs = [zig] ++ nativeBuildInputs;
      }
      // lib.optionalAttrs (rawArgs ? patches) {inherit (rawArgs) patches;}
      // lib.optionalAttrs (rawArgs ? patchFlags) {inherit (rawArgs) patchFlags;}
      // lib.optionalAttrs (rawArgs ? prePatch) {inherit (rawArgs) prePatch;}
      // lib.optionalAttrs (rawArgs ? postPatch) {inherit (rawArgs) postPatch;}
      // env
    )
    ''
      unpackPhase
      cd "$sourceRoot"
      patchPhase
      ${zigCacheScript}
      export ZIG_LOCAL_CACHE_DIR="$TMPDIR/zig-local-cache"
      zig build \
        --global-cache-dir "$ZIG_GLOBAL_CACHE_DIR" \
        --cache-dir "$ZIG_LOCAL_CACHE_DIR" \
        ${lib.escapeShellArg step} \
        ${lib.escapeShellArgs (zigBuildFlags ++ zigDefaultFlags)} \
        --summary all
      mkdir -p "$out"
      touch "$out/done"
    '';
in
  pkgs.stdenv.mkDerivation (
    commonAttrs
    // env
    // {
      inherit pname version src;

      strictDeps = true;

      nativeBuildInputs = [zig] ++ nativeBuildInputs;

      inherit buildInputs;

      dontConfigure = true;
      dontBuild = true;

      installPhase = ''
        # shell
        runHook preInstall

        ${zigCacheScript}
        export ZIG_LOCAL_CACHE_DIR="$TMPDIR/zig-local-cache"

        buildCores=1
        if [ "''${enableParallelBuilding-1}" ]; then
          buildCores="$NIX_BUILD_CORES"
        fi

        zig build \
          "-j$buildCores" \
          --global-cache-dir "$ZIG_GLOBAL_CACHE_DIR" \
          --cache-dir "$ZIG_LOCAL_CACHE_DIR" \
          ${lib.escapeShellArgs (zigBuildFlags ++ zigDefaultFlags ++ zigInstallFlags)} \
          --prefix "$out" \
          --summary all

        runHook postInstall
      '';

      doCheck = false;

      passthru =
        passthru
        // {
          inherit zig zigDeps testSteps;
          tests = (passthru.tests or {}) // lib.mapAttrs testFor testSteps;
        };

      meta =
        meta
        // {
          mainProgram = meta.mainProgram or pname;
        };
    }
  )
