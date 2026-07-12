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
pkgs: {
  pname,
  version ? "0.0.0",
  src,
  extraNativeBuildInputs ? [],
  preTest ? "",
}: let
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
    nativeBuildInputs =
      [
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
        # shell
        runHook preBuild
        ${preTest}
        ${vitestCli} run
        runHook postBuild
      '';
      installPhase = ''
        # shell
        runHook preInstall
        mkdir -p "$out"
        echo passed > "$out/result"
        runHook postInstall
      '';
    }
  );

  manifest = pkgs.stdenvNoCC.mkDerivation (
    baseAttrs
    // {
      pname = "${pname}-vitest-manifest";
      inherit version;
      buildPhase = ''
        # shell
        runHook preBuild
        ${preTest}
        ${vitestCli} list --json --static-parse > tests.json
        runHook postBuild
      '';
      installPhase = ''
        # shell
        runHook preInstall
        mkdir -p "$out"
        cp tests.json "$out/tests.json"
        runHook postInstall
      '';
    }
  );

  manifestEntries = lib.importJSON "${manifest}/tests.json";

  exactNamePattern = name: "^${lib.escapeRegex name}$";
  relativeTestFile = entry: let
    buildSourcePrefix = "/build/source/";
  in
    if lib.hasPrefix buildSourcePrefix entry.file
    then lib.removePrefix buildSourcePrefix entry.file
    else entry.file;
  projectArgs = entry:
    lib.optionals (entry ? projectName && entry.projectName != null) [
      "--project"
      entry.projectName
    ];
  caseArgs = entry:
    [
      (relativeTestFile entry)
    ]
    ++ projectArgs entry
    ++ [
      "--testNamePattern"
      (exactNamePattern entry.name)
    ];

  # Turn a string into a single nix-attr-safe slug: lowercase, separators
  # flattened to single dashes, anything not in [a-z0-9-] dropped entirely.
  slugify = name: let
    lowered = lib.toLower name;
    withSeps = lib.replaceStrings [" > "] ["--"] lowered;
    safe = lib.stringAsChars (c:
      if (builtins.match "[a-z0-9-]" c != null)
      then c
      else "-")
    withSeps;
  in
    lib.pipe safe [
      # Collapse runs of dashes to one.
      (s: lib.concatStringsSep "-" (builtins.filter (p: p != "") (lib.splitString "-" s)))
    ];

  caseId = entry: let
    slug = slugify entry.name;
    readable =
      if slug == ""
      then "case"
      else slug;
    digest = builtins.substring 0 12 (
      builtins.hashString "sha256" (
        builtins.toJSON {
          file = relativeTestFile entry;
          inherit (entry) name;
          projectName = entry.projectName or null;
        }
      )
    );
  in "${readable}-${digest}";

  cases = lib.genAttrs' manifestEntries (
    entry:
      lib.nameValuePair (caseId entry) (
        pkgs.stdenvNoCC.mkDerivation (
          baseAttrs
          // {
            pname = "${pname}-vitest-${caseId entry}";
            inherit version;
            passthru = {
              testName = entry.name;
              testFile = relativeTestFile entry;
              testProject = entry.projectName or null;
              vitestArgs = caseArgs entry;
            };
            buildPhase = ''
              # shell
              runHook preBuild
              ${preTest}
              ${vitestCli} run ${lib.escapeShellArgs (caseArgs entry)}
              runHook postBuild
            '';
            installPhase = ''
              # shell
              runHook preInstall
              mkdir -p "$out"
              echo passed > "$out/result"
              runHook postInstall
            '';
          }
        )
      )
  );
in {
  inherit all manifest cases;
}
