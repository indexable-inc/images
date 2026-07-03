{
  lib,
  # astlog-ignore: no-pkgs-in-callpackage
  pkgs,
  go,
}: let
  sanitizePackage = package: let
    raw =
      if package == "." || package == "./"
      then "root"
      else
        lib.replaceStrings
        [
          "./"
          "/"
          "."
          "*"
        ]
        [
          ""
          "-"
          "-"
          "all"
        ]
        package;
  in
    if raw == ""
    then "root"
    else raw;

  commonArgs = args: let
    src = args.src or (throw "goUnit.buildWorkspace requires src");
    canReadModuleFiles = !(lib.isDerivation src);
    modRoot = args.modRoot or ".";
    moduleRoot =
      if modRoot == "."
      then src
      else src + "/${modRoot}";
    explicitGoMod = args ? goMod;
    goMod = args.goMod or (moduleRoot + "/go.mod");
    canReadGoMod = (canReadModuleFiles || explicitGoMod) && builtins.pathExists goMod;
    goModExists = canReadGoMod || (!canReadModuleFiles && !explicitGoMod);
    missingGoModMessage = "goUnit.buildWorkspace requires ${builtins.toString goMod}";
    checkedGoMod =
      if goModExists
      then goMod
      else throw missingGoModMessage;
    explicitGoSum = args ? goSum;
    goSum = args.goSum or (moduleRoot + "/go.sum");
    requestedNoSumModule = (args ? vendorHash) && args.vendorHash == null;
    readableGoModHasRequire =
      canReadGoMod
      && lib.any (
        line: let
          compactLine = lib.replaceStrings [" " "\t"] ["" ""] line;
        in
          lib.hasPrefix "require(" compactLine
          || (lib.hasPrefix "require" compactLine && compactLine != "require")
      ) (lib.splitString "\n" (builtins.readFile checkedGoMod));
    unreadableNoSumModule = requestedNoSumModule && !canReadGoMod;
    noSumModule = requestedNoSumModule && canReadGoMod && !readableGoModHasRequire;
    goSumForBuild =
      if explicitGoSum
      then args.goSum
      else if canReadModuleFiles && builtins.pathExists goSum
      then goSum
      else null;
    goSumExists =
      if explicitGoSum
      then
        (goSumForBuild == null && noSumModule)
        || (goSumForBuild != null && builtins.pathExists goSumForBuild)
      else goSumForBuild != null || noSumModule || (!canReadModuleFiles && !requestedNoSumModule);
    missingGoSumMessage = "goUnit.buildWorkspace requires ${builtins.toString goSum}; pass vendorHash = null only for stdlib-only modules without go.sum";
    unreadableNoSumMessage = "goUnit.buildWorkspace cannot verify vendorHash = null against ${builtins.toString goMod}; pass a readable goMod or a real vendorHash";
    requireNoSumMessage = "goUnit.buildWorkspace vendorHash = null is only for stdlib-only modules without require directives";
    canReadGoSum =
      goSumForBuild != null && (canReadModuleFiles || explicitGoSum) && builtins.pathExists goSumForBuild;
    canDeriveVendorHashKey = canReadGoMod && (canReadGoSum || noSumModule);
    explicitVendorHashFile = args ? vendorHashFile;
    vendorHashFile = args.vendorHashFile or (moduleRoot + "/go-modules.nix");
    vendorHashKey =
      args.vendorHashKey or (
        if canDeriveVendorHashKey
        then
          builtins.hashString "sha256" (
            (builtins.readFile checkedGoMod)
            + "\n"
            + (
              if canReadGoSum
              then builtins.readFile goSumForBuild
              else ""
            )
          )
        else null
      );
    vendorHashes =
      args.vendorHashes or (
        if (canReadModuleFiles || explicitVendorHashFile) && builtins.pathExists vendorHashFile
        then import vendorHashFile
        else {}
      );
    vendorHash =
      args.vendorHash or (
        if vendorHashKey != null && builtins.hasAttr vendorHashKey vendorHashes
        then vendorHashes.${vendorHashKey}
        else if vendorHashKey == null
        then
          throw ''
            goUnit.buildWorkspace cannot derive a vendor hash key from this src at eval time.
            Pass vendorHash directly, or pass vendorHashKey with vendorHashes/vendorHashFile.
          ''
        else
          throw ''
            goUnit.buildWorkspace requires a vendor hash for go.mod/go.sum key ${vendorHashKey}.
            Add ${builtins.toString vendorHashFile} with:
            {
              "${vendorHashKey}" = "sha256-...";
            }
          ''
      );
  in
    assert lib.assertMsg goModExists missingGoModMessage;
    assert lib.assertMsg (!unreadableNoSumModule) unreadableNoSumMessage;
    assert lib.assertMsg (!requestedNoSumModule || !readableGoModHasRequire) requireNoSumMessage;
    assert lib.assertMsg goSumExists missingGoSumMessage; {
      pname = args.pname or "go-unit";
      version = args.version or "0.0.0";
      inherit
        src
        modRoot
        moduleRoot
        vendorHash
        vendorHashFile
        vendorHashKey
        ;
      goMod = checkedGoMod;
      goSum = goSumForBuild;
      packages = let
        packages = args.packages or ["."];
      in
        if packages == []
        then throw "goUnit.buildWorkspace requires at least one package"
        else packages;
      goToolchain = args.goToolchain or go.toolchain pkgs {version = "latest";};
      nativeBuildInputs = args.nativeBuildInputs or [];
      buildInputs = args.buildInputs or [];
      env = args.env or {};
      ldflags = args.ldflags or [];
      tags = args.tags or [];
    };

  buildPackage = args: package: let
    buildGoModule = pkgs.buildGoModule.override {go = args.goToolchain;};
  in
    buildGoModule {
      pname = "${args.pname}-${sanitizePackage package}";
      inherit
        (args)
        version
        vendorHash
        goSum
        nativeBuildInputs
        buildInputs
        env
        ldflags
        tags
        ;
      src = args.moduleRoot;
      modRoot = ".";
      subPackages = [package];
      doCheck = false;
      strictDeps = true;
      passthru.goUnit = {
        inherit
          (args)
          goSum
          goToolchain
          env
          vendorHashKey
          vendorHashFile
          ;
        inherit package;
      };
    };

  testPackage = args: package: let
    buildGoModule = pkgs.buildGoModule.override {go = args.goToolchain;};
  in
    buildGoModule {
      pname = "${args.pname}-${sanitizePackage package}-test";
      inherit
        (args)
        version
        vendorHash
        goSum
        nativeBuildInputs
        buildInputs
        env
        ldflags
        tags
        ;
      src = args.moduleRoot;
      modRoot = ".";
      subPackages = [package];
      doCheck = true;
      strictDeps = true;
      installPhase = ''
        # shell
        runHook preInstall
        mkdir -p "$out"
        touch "$out/done"
        runHook postInstall
      '';
      passthru.goUnit = {
        inherit
          (args)
          goSum
          goToolchain
          env
          vendorHashKey
          vendorHashFile
          ;
        inherit package;
      };
    };

  /**
  Build and test a locked Go module as package-shaped Nix derivations.

  Go does not expose Cargo's rustc unit graph, so callers choose the package
  patterns that deserve independent cache and test boundaries. The helper
  requires `go.mod` and a matching Nix vendor hash. Modules with external
  dependencies should also carry `go.sum`; stdlib-only modules may pass
  `vendorHash = null` without one. By default, the vendor hash comes from
  `go-modules.nix` next to local modules, keyed by the combined `go.mod` and
  `go.sum` contents when those files are visible at eval time. This keeps the
  Nix fixed-output hash in a narrow generated artifact instead of repeating it
  at every build call site. Callers may pass `vendorHash`, `vendorHashKey`,
  `vendorHashes`, or `vendorHashFile` when the owner lives somewhere else.

  Arguments:
  - `src`: filtered module source.
  - `packages`: Go package patterns to expose, default `[ "." ]`.
  - `vendorHashFile`: attrset file keyed by `vendorHashKey`, default `go-modules.nix`.
  - `goToolchain`: optional Go package, default `ix.languages.go.toolchain pkgs { version = "latest"; }`.

  Returns `packages`, `tests`, `default`, `checks`, and `sourceAudit`.
  */
  buildWorkspace = rawArgs: let
    args = commonArgs rawArgs;
    packageNames = map sanitizePackage args.packages;
    uniquePackageNames = lib.unique packageNames;
    packageAttrs = lib.listToAttrs (
      lib.zipListsWith (
        name: package: lib.nameValuePair name (buildPackage args package)
      )
      packageNames
      args.packages
    );
    testAttrs = lib.listToAttrs (
      lib.zipListsWith (
        name: package: lib.nameValuePair name (testPackage args package)
      )
      packageNames
      args.packages
    );
  in
    assert lib.assertMsg (builtins.length uniquePackageNames == builtins.length packageNames)
    "goUnit.buildWorkspace package patterns must sanitize to unique names: ${lib.concatStringsSep ", " args.packages}"; {
      packages = packageAttrs;
      tests = testAttrs;
      checks = testAttrs;
      default = packageAttrs.${builtins.head packageNames};
      sourceAudit = {
        module = {
          base = "workspace";
          scope = "module";
          relative = args.modRoot;
          lockFile =
            if args.goSum == null
            then null
            else "go.sum";
          inherit (args) vendorHashKey;
        };
      };
      inherit (args) vendorHashKey;
    };
in {
  inherit buildWorkspace;
}
