{
  lib,
  pkgs,
}: let
  fromUvHash = hash: let
    parts = lib.splitString ":" hash;
  in
    if builtins.length parts == 2
    then
      builtins.convertHash {
        hashAlgo = builtins.elemAt parts 0;
        hash = builtins.elemAt parts 1;
        toHashFormat = "sri";
      }
    else hash;

  packageKey = lockedPackage: "${lockedPackage.name}-${lockedPackage.version}";

  distributionFor = lockedPackage: distribution:
    assert lib.assertMsg (
      distribution ? url
    ) "uv.lock package ${packageKey lockedPackage} has a distribution without a url";
    assert lib.assertMsg (
      distribution ? hash
    ) "uv.lock package ${packageKey lockedPackage} distribution ${distribution.url} is missing a hash"; {
      inherit (lockedPackage) name version;
      inherit (distribution) url;
      fileName = builtins.baseNameOf distribution.url;
      hash = fromUvHash distribution.hash;
      key = packageKey lockedPackage;
    };

  allDistributionsFor = lockedPackage: let
    wheels = lockedPackage.wheels or [];
    sdist = lib.optional (lockedPackage ? sdist) lockedPackage.sdist;
  in
    map (distributionFor lockedPackage) (wheels ++ sdist);

  wheelTagsFor = fileName: let
    parts = lib.splitString "-" fileName;
    partsCount = builtins.length parts;
  in
    if lib.hasSuffix ".whl" fileName && partsCount >= 5
    then {
      python = lib.splitString "." (builtins.elemAt parts (partsCount - 3));
      abi = lib.splitString "." (builtins.elemAt parts (partsCount - 2));
      platform = lib.splitString "." (lib.removeSuffix ".whl" (builtins.elemAt parts (partsCount - 1)));
    }
    else null;

  compatiblePlatformTag = platform: tag:
    tag
    == "any"
    || (
      platform.isLinux
      && lib.hasInfix "manylinux" tag
      && (
        (platform.isx86_64 && lib.hasSuffix "x86_64" tag)
        || (platform.isAarch64 && lib.hasSuffix "aarch64" tag)
      )
    )
    || (
      platform.isDarwin
      && lib.hasPrefix "macosx" tag
      && (
        lib.hasSuffix "universal2" tag
        || (platform.isx86_64 && lib.hasSuffix "x86_64" tag)
        || (platform.isAarch64 && lib.hasSuffix "arm64" tag)
      )
    );

  compatibleWheel = {
    python,
    platform,
  }: distribution: let
    tags = wheelTagsFor distribution.fileName;
    versionParts = lib.splitString "." python.pythonVersion;
    major = builtins.elemAt versionParts 0;
    minor = builtins.elemAt versionParts 1;
    cpythonTag = "cp${major}${minor}";
    abi3CpythonTags = map (candidateMinor: "cp${major}${toString candidateMinor}") (
      lib.range 2 (lib.toInt minor)
    );
    pythonTags =
      [
        "py3"
        "py${major}"
        cpythonTag
      ]
      ++ lib.optionals (builtins.elem "abi3" tags.abi) abi3CpythonTags;
    abiTags = [
      "none"
      "abi3"
      cpythonTag
    ];
  in
    tags
    != null
    && lib.any (tag: builtins.elem tag pythonTags) tags.python
    && lib.any (tag: builtins.elem tag abiTags) tags.abi
    && lib.any (compatiblePlatformTag platform) tags.platform;

  wheelhouseDistributionsFor = {
    python,
    platform,
  }: lockedPackage: let
    wheels = map (distributionFor lockedPackage) (lockedPackage.wheels or []);
    compatibleWheels = lib.filter (compatibleWheel {inherit python platform;}) wheels;
    sdist = lib.optional (lockedPackage ? sdist) (distributionFor lockedPackage lockedPackage.sdist);
  in
    if compatibleWheels != []
    then compatibleWheels
    else sdist;

  self = {
    /**
    Parse a `uv.lock` file into normalized package and distribution metadata.

    Only locked archive distributions are fetched: registry packages with
    `wheels` and/or `sdist` entries. Local workspace packages remain in the
    source tree and are built by `uv` during the application build.

    Arguments:
    - `uvRoot`: project root containing `uv.lock`.
    - `uvLock`: optional lockfile contents override.

    Returns:
    - `raw`: parsed TOML lockfile.
    - `packages`: lockfile package entries.
    - `distributions`: all normalized archive entries with SRI hashes.
    */
    importLock = {
      uvRoot,
      uvLock ? builtins.readFile (uvRoot + "/uv.lock"),
    }: let
      raw = builtins.fromTOML uvLock;
      packages = raw.package or [];
    in {
      inherit raw packages;
      distributions = lib.concatMap allDistributionsFor packages;
    };

    /**
    Build a wheelhouse from the archive distributions pinned in `uv.lock`.

    The resulting directory contains symlinks named like the original wheel or
    sdist files. It is suitable for `uv pip install --no-index --find-links`.

    Arguments:
    - `uvRoot`: project root containing `uv.lock`.
    - `uvLock`: optional lockfile contents override.
    - `python`: interpreter whose tags select compatible wheels.
    - `platform`: host platform whose tags select compatible wheels.
    - `fetcherOpts`: per-package fetcher overrides keyed by
      `<name>-<version>`, for unusual URLs that need extra `fetchurl` flags.

    Returns a derivation with `passthru.lock` containing the parsed metadata.
    */
    buildWheelhouse = {
      uvRoot,
      uvLock ? builtins.readFile (uvRoot + "/uv.lock"),
      python ? pkgs.python3,
      platform ? pkgs.stdenv.hostPlatform,
      fetcherOpts ? {},
    }: let
      lock = self.importLock {inherit uvRoot uvLock;};
      distributions =
        lib.concatMap (wheelhouseDistributionsFor {
          inherit python platform;
        })
        lock.packages;
      fetchedDistributions =
        map (distribution: {
          inherit distribution;
          src = pkgs.fetchurl (
            {
              inherit (distribution) url hash;
              name = distribution.fileName;
            }
            // (fetcherOpts.${distribution.key} or {})
          );
        })
        distributions;
    in
      pkgs.runCommand "uv-wheelhouse"
      {
        passthru = {
          inherit distributions lock;
        };
      }
      ''
        mkdir -p "$out"
        ${lib.concatMapStringsSep "\n" (fetchedDistribution: ''
            ln -sf ${lib.escapeShellArg "${fetchedDistribution.src}"} "$out"/${lib.escapeShellArg fetchedDistribution.distribution.fileName}
          '')
          fetchedDistributions}
      '';
  };
in
  self
