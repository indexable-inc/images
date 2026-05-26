/**
  Run a Vitest browser-mode suite from an npm project inside the Nix
  sandbox, with playwright's bundled chromium.

  The helper exposes two derivations:

  - `all`: a single `runCommand` that runs `vitest run` for the whole
    suite. Cheap; use as a `checks.<name>` entry.
  - `cases.<id>`: one `runCommand` per individual `#test`, gated on
    `vitest list --json --static-parse`. Touching any case forces the
    test manifest to build, but the manifest is a single derivation so
    enumeration is one IFD for the whole project, not one per file.

  Arguments:
  - `pname`, `version`: derivation identity.
  - `src`: project root containing `package.json` and the vitest config.
  - `extraNativeBuildInputs`: extra packages on PATH (e.g. `git`).
  - `preTest`: shell code to run before `vitest`.
*/
pkgs:
{
  pname,
  version ? "0.0.0",
  src,
  extraNativeBuildInputs ? [ ],
  preTest ? "",
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

  baseAttrs = {
    inherit src npmDeps;
    strictDeps = true;
    nativeBuildInputs = [
      pkgs.nodejs
      pkgs.importNpmLock.linkNodeModulesHook
    ]
    ++ extraNativeBuildInputs;
    # Playwright in nixpkgs ships chromium under this path; the wrapper
    # checks the version subdirectories so a custom validate is noise.
    PLAYWRIGHT_BROWSERS_PATH = "${pkgs.playwright-driver.browsers}";
    PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "true";
    PLAYWRIGHT_NODEJS_PATH = "${pkgs.nodejs}/bin/node";
  };

  vitestCli = "node node_modules/vitest/vitest.mjs";

  all = pkgs.stdenvNoCC.mkDerivation (
    baseAttrs
    // {
      pname = "${pname}-vitest";
      inherit version;
      dontUnpack = false;
      buildPhase = ''
        runHook preBuild
        ${preTest}
        ${vitestCli} run
        runHook postBuild
      '';
      installPhase = ''
        mkdir -p "$out"
        echo passed > "$out/result"
      '';
    }
  );

  manifest = pkgs.stdenvNoCC.mkDerivation (
    baseAttrs
    // {
      pname = "${pname}-vitest-manifest";
      inherit version;
      buildPhase = ''
        runHook preBuild
        ${preTest}
        ${vitestCli} list --json --static-parse > tests.json
        runHook postBuild
      '';
      installPhase = ''
        mkdir -p "$out"
        cp tests.json "$out/tests.json"
      '';
    }
  );

  manifestEntries = builtins.fromJSON (builtins.readFile "${manifest}/tests.json");

  # Turn a "describe > test" string into a single nix-attr-safe slug:
  # lowercase, separators flattened to single dashes, anything not in
  # [a-z0-9-] dropped entirely.
  caseId =
    name:
    let
      lowered = lib.toLower name;
      withSeps = lib.replaceStrings [ " > " ] [ "--" ] lowered;
      safe = lib.stringAsChars (c: if (builtins.match "[a-z0-9-]" c != null) then c else "-") withSeps;
    in
    lib.pipe safe [
      # Collapse runs of dashes to one.
      (s: lib.concatStringsSep "-" (builtins.filter (p: p != "") (lib.splitString "-" s)))
    ];

  cases = lib.listToAttrs (
    map (
      entry:
      lib.nameValuePair (caseId entry.name) (
        pkgs.stdenvNoCC.mkDerivation (
          baseAttrs
          // {
            pname = "${pname}-vitest-${caseId entry.name}";
            inherit version;
            passthru.testName = entry.name;
            passthru.testFile = entry.file;
            buildPhase = ''
              runHook preBuild
              ${preTest}
              ${vitestCli} run --testNamePattern ${lib.escapeShellArg entry.name}
              runHook postBuild
            '';
            installPhase = ''
              mkdir -p "$out"
              echo passed > "$out/result"
            '';
          }
        )
      )
    ) manifestEntries
  );
in
{
  inherit all manifest cases;
}
