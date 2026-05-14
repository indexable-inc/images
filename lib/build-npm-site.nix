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
  distDir ? "dist",
  installDir ? "share/${pname}",
  extraNativeBuildInputs ? [ ],
  meta ? { },
}:
let
  npmDeps = pkgs.importNpmLock.buildNodeModules {
    npmRoot = src;
    inherit (pkgs) nodejs;
    derivationArgs = {
      strictDeps = true;
    };
  };
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
    npm run ${buildScript}
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p "$out/${installDir}"
    cp -R "${distDir}/." "$out/${installDir}/"
    runHook postInstall
  '';
})
