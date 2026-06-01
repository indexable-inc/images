{
  bunLockFor,
  errors,
}:

/**
  Build a static frontend site from a locked npm or Bun project.

  One builder for both package managers: pick with `packageManager`. Dependency
  hashes come from the lockfile (`package-lock.json` for npm, `bun.lock` for
  Bun), so updating dependencies is just `npm install` / `bun install` plus a
  commit. Dependencies are built separately and linked into the site build, so
  source-only changes do not reinstall.

  For a site that also needs a static preview server or a checkout dev server,
  use [`ix.buildSvelteSite`](svelte-site.nix), which wraps this same
  package-manager branching with those extra surfaces.

  Arguments:
  - `pname`, `version`: derivation identity.
  - `src`: project root containing `package.json` and the selected lockfile.
  - `packageManager`: `npm` or `bun`.
  - `buildScript`: package script to run for the production build.
  - `buildFlags`: arguments passed to the build script after `--`.
  - `preBuild`: shell code to run before the production build.
  - `distDir`: relative path of the build output inside `src`.
  - `installDir`: path under `$out` where the built assets are installed.
  - `installFlags`: extra `bun install` flags when `packageManager = "bun"`.
  - `extraNativeBuildInputs`: extra packages on PATH for the build.
  - `meta`: standard derivation meta.
*/
pkgs:
{
  pname,
  version ? "0.0.0",
  src,
  packageManager ? "npm",
  buildScript ? "build",
  buildFlags ? [ ],
  preBuild ? "",
  distDir ? "dist",
  installDir ? "share/${pname}",
  installFlags ? [ ],
  extraNativeBuildInputs ? [ ],
  meta ? { },
}:
let
  inherit (pkgs) lib;

  checkedPackageManager = errors.assertEnum {
    name = "ix.buildJsSite.packageManager";
    value = packageManager;
    valid = [
      "bun"
      "npm"
    ];
  };

  packageManagers = {
    bun =
      let
        bunLock = bunLockFor pkgs;
        bunNodeModules = bunLock.buildNodeModules {
          bunRoot = src;
          inherit installFlags;
          derivationArgs = {
            strictDeps = true;
          };
        };
      in
      {
        buildCommandPrefix = [
          "bun"
          "run"
        ];
        nativeBuildInputs = [
          pkgs.bun
          bunLock.nodeCompat
        ];
        derivationAttrs = {
          inherit bunNodeModules;
          configurePhase = ''
            runHook preConfigure

            patchShebangs .
            ln -s ${bunNodeModules}/node_modules node_modules
            export PATH="$PWD/node_modules/.bin:$PATH"

            runHook postConfigure
          '';
          passthru = {
            inherit bunNodeModules;
          };
        };
      };

    npm =
      let
        npmDeps = pkgs.importNpmLock.buildNodeModules {
          npmRoot = src;
          inherit (pkgs) nodejs;
          derivationArgs = {
            strictDeps = true;
          };
        };
      in
      {
        buildCommandPrefix = [
          "npm"
          "run"
        ];
        nativeBuildInputs = [
          pkgs.nodejs
          pkgs.importNpmLock.linkNodeModulesHook
        ];
        derivationAttrs = {
          inherit npmDeps;
          passthru = {
            inherit npmDeps;
          };
        };
      };
  };
  manager = packageManagers.${checkedPackageManager};

  buildCommand =
    manager.buildCommandPrefix
    ++ [ buildScript ]
    ++ lib.optional (buildFlags != [ ]) "--"
    ++ buildFlags;
in
pkgs.stdenvNoCC.mkDerivation (
  _:
  {
    inherit
      pname
      version
      src
      meta
      ;

    strictDeps = true;
    nativeBuildInputs = manager.nativeBuildInputs ++ extraNativeBuildInputs;

    buildPhase = ''
      runHook preBuild
      ${preBuild}
      ${lib.escapeShellArgs buildCommand}
      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall
      mkdir -p "$out"/${lib.escapeShellArg installDir}
      cp -R ${lib.escapeShellArg (distDir + "/.")} "$out"/${lib.escapeShellArg installDir}/
      runHook postInstall
    '';
  }
  // manager.derivationAttrs
)
