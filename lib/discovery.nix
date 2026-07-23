{
  lib,
  paths,
  importIxFor,
  mkFleetFor,
  mkVmFor,
  mkDevFor,
  ixReturn,
}: let
  inherit (import ./util/deep-merge.nix {inherit lib;}) strictList;
  inherit (import ./util/lists.nix {inherit lib;}) findDuplicatesBy;

  /**
  Walk a directory tree and return `{ <name> = { path; metadata; }; }`.
  Entries are directories containing every required file. Directories whose
  names start with `_` are skipped with their subtree. `validate` may return
  extra metadata and `outputNames` for additional duplicate claims.
  */
  discoverTree = {
    root,
    requiredFiles ? ["default.nix"],
    metadataFile ? null,
    metadataArgs ? {},
    validate ? _: {},
  }: let
    walk = path: segments: let
      entries = builtins.readDir path;
      dirs = lib.attrNames (
        lib.filterAttrs (name: type: type == "directory" && !(lib.hasPrefix "_" name)) entries
      );
      hasRequiredFiles = lib.all (file: (entries.${file} or null) == "regular") requiredFiles;
      baseMetadata =
        {
          name = lib.last segments;
          inherit segments;
          relativePath = lib.concatStringsSep "/" segments;
        }
        // lib.optionalAttrs (metadataFile != null) {
          sidecar =
            if (entries.${metadataFile} or null) == "regular"
            then import (path + "/${metadataFile}") ({inherit lib;} // metadataArgs)
            else null;
        };
      metadata =
        baseMetadata
        // validate {
          inherit path;
          metadata = baseMetadata;
        };
      entry = {
        inherit path metadata;
        claims = map (name: {
          inherit name path;
          inherit (metadata) relativePath;
        }) (lib.unique ([metadata.name] ++ (metadata.outputNames or [])));
      };
    in
      lib.optional (segments != [] && hasRequiredFiles) entry
      ++ lib.concatMap (name: walk (path + "/${name}") (segments ++ [name])) dirs;

    discovered = walk root [];
    allClaims = lib.concatMap (entry: entry.claims) discovered;
    duplicateNames = findDuplicatesBy (claim: claim.name) allClaims;
    duplicateClaims = lib.filter (claim: builtins.elem claim.name duplicateNames) allClaims;
  in
    assert lib.assertMsg (duplicateClaims == []) (
      lib.concatMapStringsSep "\n" (
        claim: "discoverTree: duplicate output name '${claim.name}' claimed by ${claim.relativePath} at ${builtins.toString claim.path}"
      )
      duplicateClaims
    );
      lib.genAttrs' discovered (
        entry: lib.nameValuePair entry.metadata.name {inherit (entry) path metadata;}
      );

  /**
  Walk `modules/<category>/<name>/` under `root` and expose every
  discovered NixOS module as an attrset of paths. Each module is a
  directory containing `default.nix`; sibling directories with their
  own `default.nix` become nested keys (so `services/minecraft/` ships
  `{ default = ./minecraft; fabric = ./minecraft/fabric; ...; mods = { bluemap = ...; }; }`).

  A directory or `.nix` file whose name starts with `_` is skipped, so
  a module can keep non-module helper data (templates, dashboards, lua)
  alongside its `default.nix` without polluting the registry.

  The walk only enumerates directories and only treats a directory as
  a module when it has its own `default.nix`. Sibling `.nix` files,
  Lua, Nu, and other resources are ignored; if a module needs them,
  `default.nix` imports them directly.
  */
  discoverModules = {root}: let
    discovered = discoverTree {
      inherit root;
      validate = {metadata, ...}: let
        inherit (metadata) segments;
        category = builtins.head segments;
        moduleSegments = builtins.tail segments;
      in
        assert lib.assertMsg (builtins.length segments > 1)
        "discoverModules: category '${category}' has its own default.nix; categories must only contain module subdirectories"; {
          inherit moduleSegments;
          name = lib.concatStringsSep "." moduleSegments;
        };
    };
    entries = builtins.attrValues discovered;
    modulePaths = map (entry: entry.metadata.moduleSegments) entries;
    hasDescendant = modulePath:
      lib.any (
        otherPath: otherPath != modulePath && lib.lists.hasPrefix modulePath otherPath
      )
      modulePaths;
    entryAsTree = entry: let
      modulePath = entry.metadata.moduleSegments;
      outputPath =
        if hasDescendant modulePath
        then modulePath ++ ["default"]
        else modulePath;
    in
      lib.setAttrByPath outputPath entry.path;
  in
    strictList (map entryAsTree entries);

  /**
  Discovered example VMs, built for a given host system. Discovery walks
  the hierarchical `examples/<category>/<name>/default.ix` layout
  (JavaScript-syntax Nix, converted through `importIxFor` — an IFD on the
  compiled `ix2nix`, since repo evals run on stock nix without
  `builtins.wasm`). Keys in the returned attrset join the category and
  name with `-`, so `examples/hermes/api-server` contributes
  `hermes-api-server`. Each config is imported with
  `{ index = { lib = ix; }; }` to match the contract examples already
  use, with `mkVm`/`mkFleet` swapped for the host-system variants so the
  wrapper derivations under `.up`/`.health`/`.replace` build for the
  requested system rather than always pinning to the default.

  Only single-VM examples (the evaluator shape: `nodes` + `planValue`)
  participate: a multi-VM example is deployed VM-by-VM with
  `ix apply .#a .#b` and an oci entry returns images, so neither has a
  lifecycle plan to aggregate. The classification is the tree itself
  (`examples/multi-vm/`, `examples/oci/`), never the converted module:
  conversion is an IFD, and the key set must stay computable by
  IFD-forbidding evals (index#4087).

  Adding an example is `mkdir examples/<category>/<name>` + edit
  `default.ix`; this aggregator picks it up on the next eval, no
  registry edits.
  */
  exampleFleetsFor = {hostSystem}: let
    indexShim = {
      lib =
        ixReturn
        // {
          mkFleet = mkFleetFor hostSystem;
          mkVm = mkVmFor hostSystem;
          mkDev = mkDevFor hostSystem;
        };
    };

    discovered = discoverTree {
      root = paths.examples;
      requiredFiles = ["default.ix"];
      validate = {metadata, ...}:
        assert lib.assertMsg (builtins.length metadata.segments == 2)
        "exampleFleetsFor: expected examples/<category>/<name>/default.ix, got examples/${metadata.relativePath}"; {
          name = lib.concatStringsSep "-" metadata.segments;
        };
    };
    # Static, tree-derived classification: `multi-vm` examples deploy VM-by-VM
    # (`ix apply .#a .#b`) and `oci` entries return images, so neither has a
    # single-VM lifecycle plan to aggregate. Deciding this from the converted
    # module would force the ix2nix converter derivation (an IFD) just to
    # compute the key set, and consumers reach these keys from evals that
    # forbid IFD (index#4087: ix's no-build lint gate probes
    # `index.packages.<system>`, whose names spread `lifecyclePackages`).
    singleVmExamples =
      lib.filterAttrs
      (_: entry: !(builtins.elem (builtins.head entry.metadata.segments) ["multi-vm" "oci"]))
      discovered;
  in
    lib.mapAttrs (
      _: entry: let
        load = importIxFor hostSystem (entry.path + "/default.ix");
        args = builtins.functionArgs load;
        value =
          if args ? index
          then load {index = indexShim;}
          else null;
      in
        assert lib.assertMsg (value != null && value ? nodes && value ? planValue)
        ("exampleFleetsFor: examples/${entry.metadata.relativePath} is not a single-VM example "
          + "(expected the evaluator shape `nodes` + `planValue`); multi-VM examples live under "
          + "examples/multi-vm/ and images under examples/oci/");
          value
    )
    singleVmExamples;
in {
  inherit
    discoverTree
    discoverModules
    exampleFleetsFor
    ;
}
