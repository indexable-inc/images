{
  lib,
  pkgs,
  go,
}:
let
  sanitizePackage =
    package:
    let
      raw =
        if package == "." || package == "./" then
          "root"
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
    if raw == "" then "root" else raw;

  commonArgs =
    args:
    let
      src = args.src or (throw "goUnit.buildWorkspace requires src");
      modRoot = args.modRoot or ".";
      moduleRoot = if modRoot == "." then src else src + "/${modRoot}";
      goMod = args.goMod or (moduleRoot + "/go.mod");
      goSum = args.goSum or (moduleRoot + "/go.sum");
      vendorHashFile = args.vendorHashFile or (moduleRoot + "/go-modules.nix");
      vendorHashKey = builtins.hashString "sha256" (
        (builtins.readFile goMod) + "\n" + (builtins.readFile goSum)
      );
      vendorHashes =
        args.vendorHashes or (if builtins.pathExists vendorHashFile then import vendorHashFile else { });
      vendorHash =
        args.vendorHash or vendorHashes.${vendorHashKey} or (throw ''
          goUnit.buildWorkspace requires a vendor hash for go.mod/go.sum key ${vendorHashKey}.
          Add ${builtins.toString vendorHashFile} with:
          {
            "${vendorHashKey}" = "sha256-...";
          }
        '');
    in
    assert lib.assertMsg (builtins.pathExists goMod)
      "goUnit.buildWorkspace requires a checked-in go.mod at ${builtins.toString goMod}";
    assert lib.assertMsg (builtins.pathExists goSum)
      "goUnit.buildWorkspace requires a checked-in go.sum lockfile at ${builtins.toString goSum}";
    {
      pname = args.pname or "go-unit";
      version = args.version or "0.0.0";
      inherit
        src
        goMod
        goSum
        modRoot
        moduleRoot
        vendorHash
        vendorHashFile
        vendorHashKey
        ;
      packages =
        let
          packages = args.packages or [ "." ];
        in
        if packages == [ ] then throw "goUnit.buildWorkspace requires at least one package" else packages;
      goToolchain = args.goToolchain or go.toolchain pkgs { version = "latest"; };
      nativeBuildInputs = args.nativeBuildInputs or [ ];
      buildInputs = args.buildInputs or [ ];
      env = args.env or { };
      ldflags = args.ldflags or [ ];
      tags = args.tags or [ ];
    };

  buildPackage =
    args: package:
    let
      buildGoModule = pkgs.buildGoModule.override { go = args.goToolchain; };
    in
    buildGoModule {
      pname = "${args.pname}-${sanitizePackage package}";
      inherit (args)
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
      subPackages = [ package ];
      doCheck = false;
      strictDeps = true;
      passthru.goUnit = {
        inherit (args)
          goSum
          goToolchain
          env
          vendorHashKey
          vendorHashFile
          ;
        inherit package;
      };
    };

  testPackage =
    args: package:
    let
      buildGoModule = pkgs.buildGoModule.override { go = args.goToolchain; };
    in
    buildGoModule {
      pname = "${args.pname}-${sanitizePackage package}-test";
      inherit (args)
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
      subPackages = [ package ];
      doCheck = true;
      strictDeps = true;
      installPhase = ''
        mkdir -p "$out"
        touch "$out/done"
      '';
      passthru.goUnit = {
        inherit (args)
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
    requires `go.mod`, `go.sum`, and a matching Nix vendor hash.
    By default, the vendor hash comes from `go-modules.nix` next to the module,
    keyed by the combined `go.mod` and `go.sum` contents. This keeps the Nix
    fixed-output hash in a narrow generated artifact instead of repeating it at
    every build call site. Callers may pass `vendorHash`, `vendorHashes`, or
    `vendorHashFile` when the owner lives somewhere else.

    Arguments:
    - `src`: filtered module source.
    - `packages`: Go package patterns to expose, default `[ "." ]`.
    - `vendorHashFile`: attrset file keyed by `vendorHashKey`, default `go-modules.nix`.
    - `goToolchain`: optional Go package, default `ix.languages.go.toolchain pkgs { version = "latest"; }`.

    Returns `packages`, `tests`, `default`, `checks`, and `sourceAudit`.
  */
  buildWorkspace =
    rawArgs:
    let
      args = commonArgs rawArgs;
      packageNames = map sanitizePackage args.packages;
      uniquePackageNames = lib.unique packageNames;
      packageAttrs = lib.listToAttrs (
        lib.zipListsWith (
          name: package: lib.nameValuePair name (buildPackage args package)
        ) packageNames args.packages
      );
      testAttrs = lib.listToAttrs (
        lib.zipListsWith (
          name: package: lib.nameValuePair name (testPackage args package)
        ) packageNames args.packages
      );
    in
    assert lib.assertMsg (builtins.length uniquePackageNames == builtins.length packageNames)
      "goUnit.buildWorkspace package patterns must sanitize to unique names: ${lib.concatStringsSep ", " args.packages}";
    {
      packages = packageAttrs;
      tests = testAttrs;
      checks = testAttrs;
      default = packageAttrs.${builtins.head packageNames};
      sourceAudit = {
        module = {
          base = "workspace";
          scope = "module";
          relative = args.modRoot;
          lockFile = "go.sum";
          inherit (args) vendorHashKey;
        };
      };
      inherit (args) vendorHashKey;
    };
in
{
  inherit buildWorkspace;
}
