{
  lib,
  paths,
  artifacts,
  mkImage,
  mkFleetFor,
  ixReturn,
}:
let
  # Subdirectories of `dir`. Used to walk images/<cat>/<name>/.
  subdirs =
    dir:
    let
      entries = builtins.readDir dir;
    in
    lib.filter (n: entries.${n} == "directory") (builtins.attrNames entries);

  # One image directory -> { <name> = pkg; <name>_<ver> = pkg; ... }.
  # Without versions.nix, the dir is a single module.
  # With versions.nix, each version is layered on top of the base module and
  # the `default` key picks which version gets the unsuffixed alias.
  imagePackages =
    name: path:
    let
      versionsPath = path + "/versions.nix";
    in
    if builtins.pathExists versionsPath then
      let
        versions = import versionsPath { inherit lib artifacts; };
        defaultVer = versions.default;
        verMods = builtins.removeAttrs versions [ "default" ];
        verPkgs = lib.mapAttrs' (
          ver: mod:
          lib.nameValuePair "${name}_${ver}" (mkImage {
            modules = [
              path
              mod
            ];
          })
        ) verMods;
        defaultKey = "${name}_${defaultVer}";
      in
      assert lib.assertMsg (builtins.hasAttr defaultKey verPkgs)
        "image '${name}': versions.nix default = \"${defaultVer}\" but no version with that key";
      verPkgs // { ${name} = verPkgs.${defaultKey}; }
    else
      { ${name} = mkImage { modules = [ path ]; }; };

  /**
    Walk `images/<category>/<name>/` under `root` and expose every
    directory as a flake package. A directory with a `versions.nix`
    sibling produces `<name>_<ver>` for each version key plus a
    `<name>` alias for the `default` version.

    `imageTests` is an optional attrset keyed by image name (matching
    the discovered package names). When an image has an entry, it is
    attached to the image derivation as `passthru.tests.eval` so
    `nix build .#<image>.passthru.tests.eval` runs it (RFC 0119).
  */
  discoverImages =
    {
      root,
      imageTests ? { },
    }:
    let
      imageCategories = lib.filter (cat: cat != "presets") (subdirs root);
      raw = lib.mergeAttrsList (
        lib.concatMap (
          cat: map (name: imagePackages name (root + "/${cat}/${name}")) (subdirs (root + "/${cat}"))
        ) imageCategories
      );
      attach =
        name: pkg:
        if imageTests ? ${name} then
          pkg
          // {
            passthru = (pkg.passthru or { }) // {
              tests = (pkg.passthru.tests or { }) // {
                eval = imageTests.${name};
              };
            };
          }
        else
          pkg;
    in
    lib.mapAttrs attach raw;

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
  discoverModules =
    { root }:
    let
      isModuleSegment = name: !(lib.hasPrefix "_" name);
      keepValue = v: builtins.isPath v || (builtins.isAttrs v && v != { });
      walk =
        dir:
        let
          entries = builtins.readDir dir;
          childDirNames = lib.filter (n: entries.${n} == "directory" && isModuleSegment n) (
            builtins.attrNames entries
          );
          hasDefault = (entries."default.nix" or null) == "regular";
          rawChildren = lib.listToAttrs (map (n: lib.nameValuePair n (walk (dir + "/${n}"))) childDirNames);
          children = lib.filterAttrs (_: keepValue) rawChildren;
        in
        if hasDefault && children == { } then
          dir
        else if hasDefault then
          children // { default = dir; }
        else
          children;
      rootEntries = builtins.readDir root;
      categoryNames = lib.filter (n: (rootEntries.${n} or "") == "directory" && isModuleSegment n) (
        builtins.attrNames rootEntries
      );
      perCategory = map (
        cat:
        let
          walked = walk (root + "/${cat}");
        in
        # A category dir without its own `default.nix` returns an attrset of
        # its children; flatten those into the top-level module registry.
        # A category dir with a `default.nix` would shadow the category name,
        # which we don't currently use, so reject it loudly.
        if builtins.isAttrs walked then
          walked
        else
          throw "discoverModules: category '${cat}' has its own default.nix; categories must only contain module subdirectories"
      ) categoryNames;
    in
    lib.mergeAttrsList perCategory;

  /**
    Discovered example fleets, built for a given host system. Discovery
    walks two layouts side by side: flat `examples/<name>/default.nix`
    and nested `examples/<category>/<name>/default.nix`. A directory is
    treated as a category when it has no `default.nix` of its own. Keys
    in the returned attrset are always the example's own name; the
    category is organizational, mirroring how `discoverImages` flattens
    `images/<cat>/<name>/` into bare names.

    Each fleet is imported with `{ index = { lib = ix; }; }` to match
    the contract examples already use, with `mkFleet` swapped for the
    host-system variant so the wrapper derivations under
    `.up`/`.health`/`.replace` build for the requested system rather
    than always pinning to the default.

    Adding an example is `mkdir examples/<category>/<name> + edit
    default.nix`; this aggregator picks it up on the next eval, no
    registry edits.
  */
  exampleFleetsFor =
    {
      hostSystem,
      # Prepend this to every example node name. The health-checks runner
      # uses "health-check-" so its lifecycle scripts cannot collide with
      # real production VMs that share the natural names (`nginx`,
      # `factions`, ...). Default empty so the regular
      # `packages.<example>-*` wrappers see no change.
      nodePrefix ? "",
    }:
    let
      indexShim = {
        lib = ixReturn // {
          mkFleet = spec: (mkFleetFor hostSystem) (spec // { inherit nodePrefix; });
        };
      };

      isExampleDir = path: builtins.pathExists (path + "/default.nix");

      topEntries = subdirs paths.examples;

      flatPairs = map (name: {
        inherit name;
        path = paths.examples + "/${name}";
      }) (lib.filter (name: isExampleDir (paths.examples + "/${name}")) topEntries);

      categoryDirs = lib.filter (name: !(isExampleDir (paths.examples + "/${name}"))) topEntries;

      nestedPairs = lib.concatMap (
        cat:
        let
          catPath = paths.examples + "/${cat}";
        in
        map (name: {
          inherit name;
          path = catPath + "/${name}";
        }) (lib.filter (name: isExampleDir (catPath + "/${name}")) (subdirs catPath))
      ) categoryDirs;
    in
    lib.listToAttrs (
      map (e: lib.nameValuePair e.name (import e.path { index = indexShim; })) (flatPairs ++ nestedPairs)
    );
in
{
  inherit
    discoverImages
    discoverModules
    exampleFleetsFor
    ;
}
