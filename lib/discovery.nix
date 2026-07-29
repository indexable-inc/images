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
  the `examples/[<category>/]<name>/default.ix` layout (the category
  level is optional for an example that is its own category)
  (JavaScript-syntax Nix, converted in-eval through `importIxFor`, i.e.
  `builtins.wasm` over the built `ix2nix-wasm` converter; repo evals run
  on the nix-ix client with `wasm-builtin`). Keys in the returned attrset join
  the category and
  name with `-`, so `examples/hermes/api-server` contributes
  `hermes-api-server`. Each config is imported with
  `{ index = { lib = ix; }; }` to match the contract examples already
  use, with `mkVm`/`mkFleet` swapped for the host-system variants so the
  wrapper derivations under `.up`/`.health`/`.replace` build for the
  requested system rather than always pinning to the default.

  Only single-VM results (the evaluator shape: `nodes` + `planValue`)
  are kept: a multi-VM example is deployed VM-by-VM with
  `ix apply .#a .#b` and an oci entry returns images, so neither has a
  lifecycle plan to aggregate. The filter forces each conversion; see
  the note at the filter for what may consume the resulting key set
  (index#4087).

  Adding an example is `mkdir examples/[<category>/]<name>` + edit
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
        assert lib.assertMsg (builtins.length metadata.segments <= 2)
        "exampleFleetsFor: expected examples/[<category>/]<name>/default.ix, got examples/${metadata.relativePath}"; {
          name = lib.concatStringsSep "-" metadata.segments;
        };
    };
  in
    # Classifying single- vs multi-VM needs the converted module (the
    # `nodes` + `planValue` shape). Conversion is in-eval now (committed
    # wasm converter, no IFD), but this attrset's KEY SET still depends on
    # every example's converted contents, so it stays out of `packages`
    # names: example-derived package fan-outs live in `legacyPackages`
    # instead (index#4087).
    lib.filterAttrs (_: fleet: fleet != null) (
      lib.mapAttrs (
        _: entry: let
          load = importIxFor hostSystem (entry.path + "/default.ix");
          args = builtins.functionArgs load;
          # An example may take flake inputs of its own, and this aggregator
          # has only `index` to give. Skipping those rather than calling them
          # is what lets an example depend on a service repo: the alternative
          # is an eval error for the whole tree the moment one does.
          # `functionArgs` marks an argument false when it has no default, so
          # this is exactly the set that would throw.
          unsuppliable =
            builtins.filter
            (name: name != "index" && !args.${name})
            (builtins.attrNames args);
          # Say so, though. A silent skip makes an example that stopped being
          # discovered look exactly like one that was never added: #4334
          # shipped two that were invisible to this aggregator from their
          # first commit, and nothing anywhere said a word (index#4343).
          #
          # `lib.warn` is the obvious spelling and the wrong one: this repo
          # and ix both set `abort-on-warn`, so it would turn every skip into
          # the tree-wide eval failure the note above exists to prevent. A
          # trace is the loudest thing left that still lets the eval finish.
          skip = reason:
          # astlog-ignore: no-deprecated-trace
            builtins.trace
            "exampleFleetsFor: skipping examples/${entry.metadata.relativePath}: ${reason}"
            null;
          value =
            if !(args ? index)
            then skip "default.ix takes no `index` argument, so there is no way to call it"
            else if unsuppliable != []
            then
              skip "default.ix requires ${
                lib.concatMapStringsSep ", " (name: "`${name}`") unsuppliable
              }, which this aggregator cannot supply"
            else load {index = indexShim;};
        in
          if value != null && value ? nodes && value ? planValue
          then value
          else null
      )
      discovered
    );
in {
  inherit
    discoverTree
    discoverModules
    exampleFleetsFor
    ;
}
