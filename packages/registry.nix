{ lib, root }:
let
  relativePath = path: lib.removePrefix "${builtins.toString root}/" (builtins.toString path);

  childDirs =
    dir:
    let
      entries = builtins.readDir dir;
    in
    map (name: dir + "/${name}") (
      lib.filter (name: entries.${name} == "directory") (builtins.attrNames entries)
    );

  dirsWithFile =
    fileName: dir:
    let
      entries = builtins.readDir dir;
      here = lib.optional ((entries.${fileName} or null) == "regular") dir;
    in
    here ++ lib.concatMap (child: dirsWithFile fileName child) (childDirs dir);

  packageDirs = dirsWithFile "package.nix" root;
  defaultPackageDirs = dirsWithFile "default.nix" root;
  packageDirsWithoutMetadata = lib.filter (
    dir: !(builtins.pathExists (dir + "/package.nix"))
  ) defaultPackageDirs;

  normalizeArgs = value: if builtins.isFunction value then value else (_: value);

  normalizePackageSet =
    id: value:
    if value == null || value == false then
      null
    else if value == true then
      {
        attrPath = [ id ];
        systems = null;
      }
    else
      value
      // {
        attrPath = value.attrPath or [ id ];
        systems = value.systems or null;
      };

  normalizeFlake =
    id: value:
    if value == null || value == false then
      null
    else if value == true then
      {
        attrName = id;
        systems = null;
      }
    else
      value
      // {
        attrName = value.attrName or id;
        systems = value.systems or null;
      };

  normalizeOverlay =
    id: value:
    if value == null || value == false then
      null
    else if value == true then
      {
        attrName = id;
        systems = null;
        callPackageArgs = _: { };
      }
    else
      value
      // {
        attrName = value.attrName or id;
        systems = value.systems or null;
        callPackageArgs = normalizeArgs (value.callPackageArgs or { });
      };

  normalizePassthruTests =
    id: value:
    if value == null || value == false then
      null
    else if value == true then
      {
        prefix = "rust-${id}";
      }
    else
      value
      // {
        prefix = value.prefix or "rust-${id}";
      };

  importMetadata =
    dir:
    let
      metadataFile = dir + "/package.nix";
      imported = import metadataFile;
      raw = if builtins.isFunction imported then imported { inherit lib; } else imported;
      id = raw.id or (throw "packages/${relativePath dir}/package.nix: missing required `id`");
    in
    raw
    // {
      inherit id;
      path = raw.path or dir;
      metadataPath = metadataFile;
      relativePath = relativePath dir;
      callPackageArgs = normalizeArgs (raw.callPackageArgs or { });
      packageSet = normalizePackageSet id (raw.packageSet or null);
      flake = normalizeFlake id (raw.flake or null);
      overlay = normalizeOverlay id (raw.overlay or null);
      inRustWorkspace = raw.inRustWorkspace or false;
      passthruTests = normalizePassthruTests id (raw.passthruTests or null);
    };

  entries = map importMetadata packageDirs;
  ids = map (entry: entry.id) entries;
  duplicateIds = lib.filter (id: builtins.length (lib.filter (candidate: candidate == id) ids) > 1) (
    lib.unique ids
  );
  byId = lib.listToAttrs (map (entry: lib.nameValuePair entry.id entry) entries);

  enabledForSystem =
    system: value:
    value != null && ((value.systems or null) == null || builtins.elem system value.systems);

  packageSetEntriesFor = system: lib.filter (entry: enabledForSystem system entry.packageSet) entries;

  flakeEntriesFor = system: lib.filter (entry: enabledForSystem system entry.flake) entries;

  overlayEntriesFor = system: lib.filter (entry: enabledForSystem system entry.overlay) entries;

  passthruTestEntriesFor =
    system:
    lib.filter (entry: enabledForSystem system entry.packageSet && entry.passthruTests != null) entries;

  rustWorkspaceEntries = lib.filter (entry: entry.inRustWorkspace) entries;
in
assert lib.assertMsg (
  duplicateIds == [ ]
) "packages/registry.nix: duplicate package ids: ${lib.concatStringsSep ", " duplicateIds}";
{
  inherit
    entries
    byId
    packageDirsWithoutMetadata
    packageSetEntriesFor
    flakeEntriesFor
    overlayEntriesFor
    passthruTestEntriesFor
    rustWorkspaceEntries
    ;
}
