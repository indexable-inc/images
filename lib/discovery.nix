{
  lib,
  paths,
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
  Discovered example VMs, built for a given host system in one load
  pass. Discovery walks the hierarchical
  `examples/<category>/<name>/default.ix` layout. Keys in the returned
  attrsets join the category and name with `-`, so
  `examples/hermes/api-server` contributes `hermes-api-server`. Each
  config is imported with `{ index = { lib = ix; }; }` to match the
  contract examples already use, with `mkVm`/`mkFleet`/`mkDev` swapped
  for the host-system variants so the wrapper derivations under
  `.up`/`.health`/`.replace` build for the requested system rather than
  always pinning to the default.

  Two views over the one pass (share it: call `examplesFor` once):

  - `fleets`: only single-VM results (the evaluator shape: `nodes` +
    `planValue`), the surface the lifecycle/health harness can drive. A
    multi-VM example returns a plain `{ nixosConfigurations = ...; }`
    merge and is applied VM-by-VM with `ix apply .#a .#b` (ix#8306: no
    fleet grouping), so it carries no lifecycle plan to aggregate.
  - `vms`: every example with `nixosConfigurations`, fleet-shaped or
    plain, normalized to `{ nixosConfigurations; systemPackages; }`
    (`systemPackages` keys each VM's NixOS toplevel as `<vm>-system`,
    the shape `mkFleet` exposed). This is the coverage surface:
    cache-push and security roots build every example VM's closure from
    it, so multi-VM examples stay eval- and build-covered.

  Adding an example is `mkdir examples/<category>/<name>` + edit
  `default.ix`; this aggregator picks it up on the next eval, no
  registry edits.
  */
  examplesFor = {hostSystem}: let
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
        "examplesFor: expected examples/<category>/<name>/default.ix, got examples/${metadata.relativePath}"; {
          name = lib.concatStringsSep "-" metadata.segments;
        };
    };

    values = lib.filterAttrs (_: value: value != null) (
      lib.mapAttrs (
        _: entry: let
          load = import (entry.path + "/default.ix");
          args = builtins.functionArgs load;
        in
          # Non-VM configs (the oci images take `index` but return a
          # derivation; nixos/switch-multi takes `nixpkgs`) fall out via
          # the shape filters below or this arg check.
          if args ? index
          then load {index = indexShim;}
          else null
      )
      discovered
    );

    toplevelPackages = nixosConfigurations:
      lib.mapAttrs' (
        name: cfg: lib.nameValuePair "${name}-system" cfg.config.system.build.toplevel
      )
      nixosConfigurations;
  in {
    fleets = lib.filterAttrs (_: value: value ? nodes && value ? planValue) values;
    vms =
      lib.mapAttrs (_: value: {
        inherit (value) nixosConfigurations;
        systemPackages = value.systemPackages or (toplevelPackages value.nixosConfigurations);
      })
      (lib.filterAttrs (_: value: value ? nixosConfigurations) values);
  };

  exampleFleetsFor = args: (examplesFor args).fleets;
in {
  inherit
    discoverTree
    discoverModules
    examplesFor
    exampleFleetsFor
    ;
}
