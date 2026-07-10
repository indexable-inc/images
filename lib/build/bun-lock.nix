{
  lib,
  # astlog-ignore: no-pkgs-in-callpackage
  pkgs,
}: let
  inherit
    (builtins)
    fromJSON
    readFile
    ;

  getName = package: package.name or "unknown";
  getVersion = package: package.version or "0.0.0";

  lockFileFor = {
    bunRoot ? null,
    bunLock ? readFile (bunRoot + "/bun.lock"),
  }:
    pkgs.writeText "bun.lock" bunLock;

  packageFileFor = {
    bunRoot ? null,
    packageJson ? readFile (bunRoot + "/package.json"),
  }:
    pkgs.writeText "package.json" packageJson;

  packageFor = {
    bunRoot ? null,
    packageJson ? readFile (bunRoot + "/package.json"),
  }:
    fromJSON packageJson;

  bunNodeCompatFor = bun:
    pkgs.runCommand "${bun.pname or "bun"}-node-compat-${bun.version or "unknown"}" {} ''
      mkdir -p "$out/bin"
      ln -s ${lib.getExe bun} "$out/bin/node"
    '';
in
  lib.fix (self: {
    /**
    Tiny compatibility package that exposes `bin/node` as a symlink to Bun.

    Some npm package executables still use `#!/usr/bin/env node`; putting this
    package on PATH makes those scripts execute under Bun without adding Node.js
    to the build environment.
    */
    nodeCompat = bunNodeCompatFor pkgs.bun;

    /**
    Generate normalized package metadata from a Bun text lockfile.

    The derivation parses `bun.lock` with Bun and writes JSON containing each
    npm registry tarball URL, its SRI integrity hash, and the cache path Bun
    expects. This is the explicit IFD boundary used by `buildCache`.
    */
    generateLockJson = {
      bunRoot ? null,
      bunLock ? readFile (bunRoot + "/bun.lock"),
      registryUrl ? "https://registry.npmjs.org",
    }:
      pkgs.runCommand "bun-lock-packages.json"
      {
        nativeBuildInputs = [pkgs.bun];
      }
      ''
        bun ${./bun-lock-to-json.js} ${
          lockFileFor {inherit bunRoot bunLock;}
        } ${lib.escapeShellArg registryUrl} > "$out"
      '';

    /**
    Import normalized Bun package metadata from `generateLockJson`.

    Returns `{ packages = [ ... ]; }`, where every package includes `name`,
    `version`, `url`, `integrity`, and `cachePath`.
    */
    importLock = args: lib.importJSON (self.generateLockJson args);

    /**
    Build a Bun install cache from `bun.lock` without a caller-provided hash.

    Each registry package is fetched with the integrity value recorded in the
    lockfile, then unpacked into the cache directory shape Bun accepts for
    `bun install --offline`.
    */
    buildCache = {
      bunRoot ? null,
      bunLock ? readFile (bunRoot + "/bun.lock"),
      packageJson ? readFile (bunRoot + "/package.json"),
      registryUrl ? "https://registry.npmjs.org",
      fetcherOpts ? {},
    }: let
      package = fromJSON packageJson;
      lock = self.importLock {inherit bunRoot bunLock registryUrl;};
      fetchedPackages =
        map (lockedPackage: {
          inherit (lockedPackage) cachePath;
          src = pkgs.fetchurl (
            {
              inherit (lockedPackage) url;
              hash = lockedPackage.integrity;
            }
            // (fetcherOpts.${lockedPackage.key} or {})
          );
        })
        lock.packages;
    in
      pkgs.runCommand "${getName package}-${getVersion package}-bun-cache"
      {
        nativeBuildInputs = [
          pkgs.gnutar
          pkgs.gzip
        ];
        passthru = {
          inherit lock;
        };
      }
      ''
        mkdir -p "$out/cache"
        ${lib.concatMapStringsSep "\n" (lockedPackage: ''
            mkdir -p "$out/cache/${lockedPackage.cachePath}"
            tar -xzf ${lockedPackage.src} --strip-components=1 -C "$out/cache/${lockedPackage.cachePath}"
          '')
          fetchedPackages}
      '';

    /**
    Build `node_modules` for a Bun project from `package.json` and `bun.lock`.

    The dependency cache is derived from the lockfile, copied to a writable
    temporary directory, then consumed with `bun install --offline`. Callers do
    not provide an npm-style dependency hash.
    */
    buildNodeModules = {
      bunRoot ? null,
      packageJson ? readFile (bunRoot + "/package.json"),
      bunLock ? readFile (bunRoot + "/bun.lock"),
      bun ? pkgs.bun,
      registryUrl ? "https://registry.npmjs.org",
      fetcherOpts ? {},
      installFlags ? [],
      derivationArgs ? {},
    }: let
      package = packageFor {inherit bunRoot packageJson;};
      nodeCompat = bunNodeCompatFor bun;
      packageFile = packageFileFor {inherit bunRoot packageJson;};
      lockFile = lockFileFor {inherit bunRoot bunLock;};
      bunCache = self.buildCache {
        inherit
          bunRoot
          bunLock
          packageJson
          registryUrl
          fetcherOpts
          ;
      };
      bunInstallFlags = lib.escapeShellArgs (
        [
          "--frozen-lockfile"
          "--offline"
          "--no-progress"
        ]
        ++ installFlags
      );
    in
      pkgs.stdenvNoCC.mkDerivation (
        {
          pname = derivationArgs.pname or "${getName package}-node-modules";
          version = derivationArgs.version or getVersion package;

          dontUnpack = true;
          strictDeps = true;

          buildPhase = ''
            # shell
            runHook preBuild

            cp ${packageFile} package.json
            cp ${lockFile} bun.lock

            export HOME="$TMPDIR"
            export BUN_INSTALL_CACHE_DIR="$TMPDIR/bun-cache"
            mkdir -p "$BUN_INSTALL_CACHE_DIR"
            cp -R ${bunCache}/cache/. "$BUN_INSTALL_CACHE_DIR/"
            chmod -R u+w "$BUN_INSTALL_CACHE_DIR"

            bun install ${bunInstallFlags}
            patchShebangs node_modules

            runHook postBuild
          '';

          installPhase = ''
            # shell
            runHook preInstall

            mkdir -p "$out"
            cp package.json bun.lock "$out/"
            mv node_modules "$out/"

            runHook postInstall
          '';

          passthru = {
            inherit bunCache;
          };
        }
        // derivationArgs
        // {
          nativeBuildInputs =
            [
              bun
              nodeCompat
            ]
            ++ (derivationArgs.nativeBuildInputs or []);
          passthru =
            {
              inherit bunCache nodeCompat;
            }
            // (derivationArgs.passthru or {});
        }
      );
  })
