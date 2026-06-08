{ lib, root }:
let
  inherit (import ../lib/util/lists.nix { inherit lib; }) findDuplicates;

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

  allowedMetadataKeys = [
    "flake"
    "id"
    "inRustWorkspace"
    "overlay"
    "packageSet"
    "passthruTests"
    "path"
    "updateScript"
  ];

  assertKnownKeys =
    label: allowedKeys: value:
    let
      unknownKeys = lib.subtractLists allowedKeys (builtins.attrNames value);
    in
    assert lib.assertMsg (
      unknownKeys == [ ]
    ) "${label}: unsupported keys: ${lib.concatStringsSep ", " unknownKeys}";
    value;

  # Optional, system-scoped target descriptor: null/false disables it, true takes
  # the package id as the selector, and an attrset overrides the named selector
  # key and/or `systems`. packageSet selects by `attrPath`, flake/overlay by
  # `attrName`.
  normalizeTarget =
    {
      name,
      key,
      default,
      extraKeys ? [ ],
    }:
    label: id: value:
    if value == null || value == false then
      null
    else if value == true then
      {
        ${key} = default id;
        systems = null;
      }
    else
      assertKnownKeys "${label}: ${name}" (
        [
          key
          "systems"
        ]
        ++ extraKeys
      ) value
      // {
        ${key} = value.${key} or (default id);
        systems = value.systems or null;
      };

  normalizePackageSet = normalizeTarget {
    name = "packageSet";
    key = "attrPath";
    default = id: [ id ];
  };

  normalizeFlake = normalizeTarget {
    name = "flake";
    key = "attrName";
    default = lib.id;
  };

  normalizeOverlay = normalizeTarget {
    name = "overlay";
    key = "attrName";
    default = lib.id;
    extraKeys = [ "build" ];
  };

  normalizePassthruTests =
    label: id: value:
    if value == null || value == false then
      null
    else if value == true then
      {
        prefix = "rust-${id}";
      }
    else
      assertKnownKeys "${label}: passthruTests" [ "prefix" ] value
      // {
        prefix = value.prefix or "rust-${id}";
      };

  importMetadata =
    dir:
    let
      metadataFile = dir + "/package.nix";
      imported = import metadataFile;
      raw = if builtins.isFunction imported then imported { inherit lib; } else imported;
      label = "packages/${relativePath dir}/package.nix";
      id = raw.id or (throw "packages/${relativePath dir}/package.nix: missing required `id`");
    in
    assertKnownKeys label allowedMetadataKeys raw
    // {
      inherit id;
      path = raw.path or dir;
      metadataPath = metadataFile;
      relativePath = relativePath dir;
      packageSet = normalizePackageSet label id (raw.packageSet or null);
      flake = normalizeFlake label id (raw.flake or null);
      overlay = normalizeOverlay label id (raw.overlay or null);
      inRustWorkspace = raw.inRustWorkspace or false;
      passthruTests = normalizePassthruTests label id (raw.passthruTests or null);
      # `updateScript = true` marks a package that exposes a
      # `passthru.updateScript` (e.g. a pinned prebuilt binary that tracks an
      # upstream "latest" pointer). The generated `update` app runs every
      # flagged package's updater; see lib/per-system.nix.
      updateScript = raw.updateScript or false;
    };

  entries = map importMetadata packageDirs;
  ids = map (entry: entry.id) entries;
  duplicateIds = findDuplicates ids;
  byId = lib.genAttrs' entries (entry: lib.nameValuePair entry.id entry);

  enabledForSystem =
    system: value:
    value != null && ((value.systems or null) == null || builtins.elem system value.systems);

  packageSetEntriesFor = system: lib.filter (entry: enabledForSystem system entry.packageSet) entries;

  flakeEntriesFor = system: lib.filter (entry: enabledForSystem system entry.flake) entries;

  overlayEntriesFor = system: lib.filter (entry: enabledForSystem system entry.overlay) entries;

  # Packages that expose a `passthru.updateScript`, restricted to those actually
  # built for `system` (the flake package-set path is where `updateScript` is
  # bound). Drives the generated `update` aggregator.
  updateScriptEntriesFor =
    system: lib.filter (entry: entry.updateScript) (packageSetEntriesFor system);

  passthruTestEntriesFor =
    system:
    lib.filter (
      entry:
      entry.passthruTests != null
      && (
        if entry.packageSet != null then enabledForSystem system entry.packageSet else entry.inRustWorkspace
      )
    ) entries;

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
    updateScriptEntriesFor
    passthruTestEntriesFor
    rustWorkspaceEntries
    ;
}
