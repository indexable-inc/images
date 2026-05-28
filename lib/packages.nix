{
  lib,
  packageRegistry,
  ixSpecialArgs,
  cargoUnitFor,
  goUnitFor,
  rustWorkspaceFor,
  cliArtifacts,
  clippy-fork,
}:
pkgs:
let
  packageSystem = pkgs.stdenv.hostPlatform.system;
  ixForPackages = ixSpecialArgs // {
    inherit pkgs;
    # Rebind the language unit builders to the caller's pkgs so repo
    # packages built through packageSetFor compile for the host system
    # instead of the x86_64-linux pkgs the top-level ixSpecialArgs bundle
    # is bound to.
    cargoUnit = cargoUnitFor pkgs;
    goUnit = goUnitFor pkgs;
    rustWorkspace = rustWorkspaceFor pkgs;
  };
  context = {
    inherit
      pkgs
      packageSystem
      cliArtifacts
      clippy-fork
      ixForPackages
      ;
    ix = ixForPackages;
  };
  mergePackageTrees =
    left: right:
    lib.foldl' (
      acc: name:
      let
        rightValue = right.${name};
      in
      if builtins.hasAttr name acc then
        let
          leftValue = acc.${name};
        in
        if
          builtins.isAttrs leftValue
          && builtins.isAttrs rightValue
          && !(lib.isDerivation leftValue)
          && !(lib.isDerivation rightValue)
        then
          acc // { ${name} = mergePackageTrees leftValue rightValue; }
        else
          throw "packageSetFor: duplicate package attr path segment `${name}`"
      else
        acc // { ${name} = rightValue; }
    ) left (builtins.attrNames right);
  buildEntry =
    entry:
    let
      autoArgs = pkgs // context // { inherit entry; };
    in
    lib.callPackageWith autoArgs entry.path { };
  packageTreeFor = entry: lib.setAttrByPath entry.packageSet.attrPath (buildEntry entry);
in
lib.foldl' mergePackageTrees { } (
  map packageTreeFor (packageRegistry.packageSetEntriesFor packageSystem)
)
