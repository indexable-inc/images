/**
  Build a static frontend site from an npm project.

  Dependency hashes come from `package-lock.json`, so updating
  dependencies is just `npm install` plus a commit. Dependencies are
  built separately and linked into the site build, so source-only
  changes do not rerun `npm install`.

  Arguments:
  - `pname`, `version`: derivation identity.
  - `src`: project root containing `package.json` and `package-lock.json`.
  - `buildScript`: npm script to run for the production build.
  - `buildFlags`: arguments passed to the build script after `--`.
  - `preBuild`: shell code to run before the npm build.
  - `distDir`: relative path of the build output inside `src`.
  - `installDir`: path under `$out` where the built assets are installed.
  - `extraNativeBuildInputs`: extra packages on PATH for the build.
  - `meta`: standard derivation meta.
*/
pkgs:
{
  pname,
  version ? "0.0.0",
  src,
  buildScript ? "build",
  buildFlags ? [ ],
  preBuild ? "",
  distDir ? "dist",
  installDir ? "share/${pname}",
  extraNativeBuildInputs ? [ ],
  meta ? { },
}:
let
  inherit (pkgs) lib;

  npmDeps = pkgs.importNpmLock.buildNodeModules {
    npmRoot = src;
    inherit (pkgs) nodejs;
    derivationArgs = {
      strictDeps = true;
    };
  };
  buildCommand = [
    "npm"
    "run"
    buildScript
  ]
  ++ lib.optional (buildFlags != [ ]) "--"
  ++ buildFlags;
in
pkgs.stdenvNoCC.mkDerivation (_: {
  inherit
    pname
    version
    src
    npmDeps
    meta
    ;

  strictDeps = true;

  nativeBuildInputs = [
    pkgs.nodejs
    pkgs.importNpmLock.linkNodeModulesHook
  ]
  ++ extraNativeBuildInputs;

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
})
