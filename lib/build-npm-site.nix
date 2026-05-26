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
  - `serve`: install a `$out/bin/<pname>` wrapper that runs `miniserve`
    against the built assets, so `nix run .#<name>` previews the site
    on `http://127.0.0.1:8080/` by default.
  - `serveRoutePrefix`: URL prefix the wrapper should mount the assets
    under, matching the SvelteKit `paths.base` the build was compiled
    with. Defaults to `/`.
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
  serve ? false,
  serveRoutePrefix ? "/",
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

  routePrefixFlag =
    lib.optionalString (serveRoutePrefix != "/" && serveRoutePrefix != "")
      " --route-prefix ${lib.escapeShellArg serveRoutePrefix}";
in
pkgs.stdenvNoCC.mkDerivation (_: {
  inherit
    pname
    version
    src
    npmDeps
    ;

  meta = meta // lib.optionalAttrs serve { mainProgram = pname; };

  strictDeps = true;

  nativeBuildInputs = [
    pkgs.nodejs
    pkgs.importNpmLock.linkNodeModulesHook
  ]
  ++ lib.optional serve pkgs.makeBinaryWrapper
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
    ${lib.optionalString serve ''
      mkdir -p "$out/bin"
      makeWrapper ${lib.getExe pkgs.miniserve} "$out/bin/${pname}" \
        --add-flags "--index index.html --interfaces 127.0.0.1 --port 8080${routePrefixFlag} $out/${installDir}"
    ''}
    runHook postInstall
  '';
})
