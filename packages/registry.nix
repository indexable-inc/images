{
  lib,
  root,
  # `findDuplicates` from lib/util/lists.nix, threaded in by callers (this file
  # is the package-registry bootstrap, so it cannot reach the assembled `ix`).
  findDuplicates,
}: let
  relativePath = path: lib.removePrefix "${toString root}/" (toString path);

  childDirs = dir: let
    entries = builtins.readDir dir;
  in
    map (name: dir + "/${name}") (
      lib.filter (name: entries.${name} == "directory") (builtins.attrNames entries)
    );

  dirsWithFile = fileName: dir: let
    entries = builtins.readDir dir;
    here = lib.optional ((entries.${fileName} or null) == "regular") dir;
  in
    here ++ lib.concatMap (child: dirsWithFile fileName child) (childDirs dir);

  packageDirs = dirsWithFile "package.nix" root;
  defaultPackageDirs = dirsWithFile "default.nix" root;
  packageDirsWithoutMetadata =
    lib.filter (
      dir: !(builtins.pathExists (dir + "/package.nix"))
    )
    defaultPackageDirs;

  allowedMetadataKeys = [
    "cross"
    "flake"
    "id"
    "inRustWorkspace"
    "mirror"
    "overlay"
    "packageSet"
    "passthruTests"
    "path"
    "updateScript"
  ];

  assertKnownKeys = label: allowedKeys: value: let
    unknownKeys = lib.subtractLists allowedKeys (builtins.attrNames value);
  in
    assert lib.assertMsg (
      unknownKeys == []
    ) "${label}: unsupported keys: ${lib.concatStringsSep ", " unknownKeys}"; value;

  # Optional, system-scoped target descriptor: null/false disables it, true takes
  # the package id as the selector, and an attrset overrides the named selector
  # key and/or `systems`. packageSet selects by `attrPath`, flake/overlay by
  # `attrName`.
  normalizeTarget = {
    name,
    key,
    default,
    extraKeys ? [],
  }: label: id: value:
    if value == null || value == false
    then null
    else if value == true
    then {
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
      )
      value
      // {
        ${key} = value.${key} or (default id);
        systems = value.systems or null;
      };

  normalizePackageSet = normalizeTarget {
    name = "packageSet";
    key = "attrPath";
    default = id: [id];
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
    extraKeys = ["build"];
  };

  normalizeCross = label: id: value: let
    # Apple silicon only: the fleet has no Intel-Mac users, so the second
    # triple would double cross-build cost for artifacts nobody pulls. A
    # package that needs it opts in via `cross.targets`.
    defaultTargets = ["aarch64-apple-darwin"];
    normalized =
      if value == null || value == false
      then null
      else if value == true
      then {}
      else
        assertKnownKeys "${label}: cross" [
          "attrName"
          "exposeNativeDarwin"
          "systems"
          "targets"
        ]
        value;
  in
    if normalized == null
    then null
    else
      normalized
      // {
        attrName = normalized.attrName or id;
        exposeNativeDarwin = normalized.exposeNativeDarwin or true;
        systems = normalized.systems or null;
        targets = normalized.targets or defaultTargets;
      };

  normalizePassthruTests = label: id: value:
    if value == null || value == false
    then null
    else if value == true
    then {
      prefix = "rust-${id}";
    }
    else
      assertKnownKeys "${label}: passthruTests" ["prefix"] value
      // {
        prefix = value.prefix or "rust-${id}";
      };

  # GitHub caps a topic at 50 chars of lowercase alphanumerics and hyphens,
  # starting alphanumeric (what `PUT /repos/{repo}/topics` accepts).
  validTopic = topic: builtins.match "[a-z0-9][a-z0-9-]{0,49}" topic != null;

  # Opt-in standalone mirror repo (packages/mirror + the mirror-sync
  # workflow): `repo` is the GitHub `owner/name` the generated tree is
  # snapshot-synced into. `description` and `topics` are REQUIRED -- they are
  # the mirror repo's public About sidebar, seeded when CI creates the repo
  # and kept in sync on every push to main by the repo-metadata workflow
  # (.github/workflows/repo-metadata.yml), so a published mirror can never
  # show GitHub's "No description, website, or topics provided." `homepage`
  # is optional and defaults (in lib/default.nix) to the package's tree in
  # this monorepo, the source of truth a visitor should find. Absent/false =
  # no mirror.
  normalizeMirror = label: value:
    if value == null || value == false
    then null
    else
      assertKnownKeys "${label}: mirror" [
        "description"
        "homepage"
        "repo"
        "topics"
      ]
      value
      // {
        repo = value.repo or (throw "${label}: mirror.repo is required");
        description = let
          description =
            value.description
            or (throw "${label}: mirror.description is required (it becomes the mirror repo's GitHub description)");
        in
          assert lib.assertMsg (
            description != ""
          ) "${label}: mirror.description must not be empty"; description;
        homepage = value.homepage or null;
        topics = let
          topics =
            value.topics
            or (throw "${label}: mirror.topics is required (at least one GitHub topic for the mirror repo)");
          invalid = lib.filter (topic: !validTopic topic) topics;
        in
          assert lib.assertMsg (
            topics != []
          ) "${label}: mirror.topics must list at least one topic";
          assert lib.assertMsg (
            invalid == []
          ) "${label}: mirror.topics entries must be 1-50 lowercase alphanumeric/hyphen characters, starting alphanumeric: ${lib.concatStringsSep ", " invalid}"; topics;
      };

  normalizeRustWorkspace = label: value:
    if value == null || value == false
    then null
    else if value == true
    then {
      systems = null;
    }
    else
      assertKnownKeys "${label}: inRustWorkspace" ["systems"] value
      // {
        systems = value.systems or null;
      };

  importMetadata = dir: let
    metadataFile = dir + "/package.nix";
    imported = import metadataFile;
    raw =
      if builtins.isFunction imported
      then imported {inherit lib;}
      else imported;
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
      inRustWorkspace = normalizeRustWorkspace label (raw.inRustWorkspace or null);
      cross = normalizeCross label id (raw.cross or null);
      mirror = normalizeMirror label (raw.mirror or null);
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

  enabledForSystem = system: value:
    value != null && ((value.systems or null) == null || builtins.elem system value.systems);

  packageSetEntriesFor = system: lib.filter (entry: enabledForSystem system entry.packageSet) entries;

  flakeEntriesFor = system: lib.filter (entry: enabledForSystem system entry.flake) entries;

  overlayEntriesFor = system: lib.filter (entry: enabledForSystem system entry.overlay) entries;

  crossEntriesFor = system: lib.filter (entry: enabledForSystem system entry.cross) entries;

  # Packages that expose a `passthru.updateScript`, restricted to those actually
  # built for `system` (the flake package-set path is where `updateScript` is
  # bound). Drives the generated `update` aggregator.
  updateScriptEntriesFor = system: lib.filter (entry: entry.updateScript) (packageSetEntriesFor system);

  passthruTestEntriesFor = system:
    lib.filter (
      entry:
        entry.passthruTests
        != null
        && (
          if entry.packageSet != null
          then enabledForSystem system entry.packageSet
          else enabledForSystem system entry.inRustWorkspace
        )
    )
    entries;

  mirrorEntries = lib.filter (entry: entry.mirror != null) entries;

  rustWorkspaceEntries = lib.filter (entry: entry.inRustWorkspace != null) entries;

  rustWorkspaceEntriesFor = system: lib.filter (entry: enabledForSystem system entry.inRustWorkspace) rustWorkspaceEntries;
in
  assert lib.assertMsg (
    duplicateIds == []
  ) "packages/registry.nix: duplicate package ids: ${lib.concatStringsSep ", " duplicateIds}"; {
    inherit
      entries
      byId
      packageDirsWithoutMetadata
      packageSetEntriesFor
      flakeEntriesFor
      overlayEntriesFor
      crossEntriesFor
      mirrorEntries
      updateScriptEntriesFor
      passthruTestEntriesFor
      rustWorkspaceEntries
      rustWorkspaceEntriesFor
      ;
  }
