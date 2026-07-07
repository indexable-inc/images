# Vendoring for the cargo-unit / buildPackage pipeline: turn a `Cargo.lock` into
# a package-shaped vendor directory (one entry per dependency) plus the cargo
# config script that points cargo at it. Owns the lockfile-path coercion and all
# registry/git source fetching. Pure function of its inputs; nothing here reads
# build policy or toolchain.
{
  lib,
  pkgs,
  writePythonApplication,
  # List helpers, threaded in rather than imported across dirs.
  lists,
  # Flip `allowSubstitutes` back on for the trivial-builder config files and the
  # linkFarm vendor dir; threaded from lib rather than imported across dirs. See
  # its doc comment.
  evalTimeSubstitutable,
}: let
  inherit
    (builtins)
    attrNames
    attrValues
    deepSeq
    elemAt
    filter
    getAttr
    hasAttr
    match
    toJSON
    ;

  inherit
    (lib)
    join
    subtractLists
    ;

  inherit (lists) findDuplicatesBy;

  joinLines = join "\n";

  # A `cargoLock` reference is either `{ lockFile = <path>; }` or a bare path.
  cargoLockFile = cargoLock: cargoLock.lockFile or cargoLock;

  dependencyPackages = cargoLock: let
    lock = lib.importTOML (cargoLockFile cargoLock);
  in
    filter (pkg: pkg ? source) (lock.package or []);

  hasGitSource = pkg: lib.hasPrefix "git+" pkg.source;

  gitPackages = cargoLock: filter hasGitSource (dependencyPackages cargoLock);

  packageSourceKey = pkg: "${pkg.source}#${pkg.name}@${pkg.version}";

  # Both registry shapes resolve to the same CDN artifact. `static.crates.io` is
  # the direct CloudFront URL cargo's sparse protocol uses; the older
  # `api.crates.io/api/v1/crates/.../download` endpoint just 302s here and, as
  # of 2026-05, rejects curl's default User-Agent with HTTP 403.
  registryDownloadUrls = let
    cratesIoDownloadUrl = pkg: "https://static.crates.io/crates/${pkg.name}/${pkg.name}-${pkg.version}.crate";
  in {
    "registry+https://github.com/rust-lang/crates.io-index" = cratesIoDownloadUrl;
    "sparse+https://index.crates.io/" = cratesIoDownloadUrl;
  };

  parseGitSource = source: let
    parts = match ''git\+([^?]+)(\?(rev|tag|branch)=([^#]*))?#(.*)'' source;
  in
    if parts == null
    then throw "rust: cannot parse git source string `${source}` from Cargo.lock"
    else {
      url = elemAt parts 0;
      refType = elemAt parts 2;
      ref = elemAt parts 3;
      sha = elemAt parts 4;
    };

  vendorConfigScript = {
    cargoExtraConfig,
    cargoLock,
    vendorDir,
  }: let
    cargoExtraConfigFile = evalTimeSubstitutable (pkgs.writeText "cargo-extra-config.toml" cargoExtraConfig);

    gitSources = lib.unique (
      map (pkg: parseGitSource pkg.source // {inherit (pkg) source;}) (gitPackages cargoLock)
    );

    # The default vendored-sources config, emitted when the vendor dir carries
    # no `.cargo/config.toml` of its own (the aggregate `linkFarm` never does).
    vendorConfigFile = evalTimeSubstitutable (pkgs.writeText "cargo-vendor-config.toml" ''
      [source.crates-io]
      replace-with = "vendored-sources"

      [source.vendored-sources]
      directory = "${vendorDir}"
    '');

    # One `[source."<git>"]` table per git dependency, each replacing that git
    # source with the vendored copy. Rendered at eval time and `cat` in, rather
    # than printf'd table by table in the builder.
    gitSourceBlock = git:
      joinLines (
        [
          ''[source."${git.source}"]''
          "git = ${toJSON git.url}"
        ]
        ++ lib.optional (git.refType != null) "${git.refType} = ${toJSON git.ref}"
        ++ [''replace-with = "vendored-sources"'']
      );
    gitSourceConfigFile = evalTimeSubstitutable (pkgs.writeText "cargo-git-sources.toml" (
      lib.concatMapStringsSep "\n\n" gitSourceBlock gitSources + "\n"
    ));
  in
    ''
      export CARGO_HOME="$TMPDIR/cargo-home"
      mkdir -p "$CARGO_HOME"

      if [ -f "${vendorDir}/.cargo/config.toml" ]; then
        sed 's|directory = "cargo-vendor-dir"|directory = "${vendorDir}"|' \
          "${vendorDir}/.cargo/config.toml" > "$CARGO_HOME/config.toml"
      else
        cat ${vendorConfigFile} > "$CARGO_HOME/config.toml"
      fi
    ''
    + lib.optionalString (gitSources != []) ''

      printf '\n' >> "$CARGO_HOME/config.toml"
      cat ${gitSourceConfigFile} >> "$CARGO_HOME/config.toml"
    ''
    + lib.optionalString (cargoExtraConfig != "") ''

      printf '\n' >> "$CARGO_HOME/config.toml"
      cat ${cargoExtraConfigFile} >> "$CARGO_HOME/config.toml"
    '';

  # One package-shaped vendored source directory per dependency, keyed by
  # `packageSourceKey`, plus the aggregate `linkFarm` vendor dir that symlinks
  # each entry under `<name>-<version>`. Registry crates are fetched from
  # `static.crates.io`; git crates are fetched and reduced to the single
  # referenced package. Vendor resolution stays lazy, so lockfile-only consumers
  # never force these derivations.
  mkVendor = {
    cargoLock,
    outputHashes,
    sourceOverrides,
  }: let
    packages = dependencyPackages cargoLock;

    vendorSources = let
      checkedOutputHashes = let
        gitPackageSources = map (pkg: pkg.source) (gitPackages cargoLock);
        outputHashKeys = attrNames outputHashes;

        missing = subtractLists outputHashKeys gitPackageSources;
        unused = subtractLists gitPackageSources outputHashKeys;
      in
        assert lib.assertMsg (missing == []) ''
          outputHashes is missing hashes for git source strings in Cargo.lock: ${join ", " missing}
          Key each git hash by the exact Cargo.lock source string, for example:
          outputHashes."git+https://github.com/owner/repo#rev" = "sha256-...";
        '';
        assert lib.assertMsg (unused == []) ''
          outputHashes contains keys that are not git source strings in Cargo.lock: ${join ", " unused}
          Key each git hash by the exact Cargo.lock source string, for example:
          outputHashes."git+https://github.com/owner/repo#rev" = "sha256-...";
        ''; outputHashes;
      # Flatten workspace inheritance in a vendored Cargo.toml before rustc sees it.
      # Vendored from nixpkgs so a downstream rename of
      # `pkgs/build-support/rust/replace-workspace-values.py` doesn't surface as a
      # `readFile` error here; `ix.writePythonApplication` also runs ty on the body
      # at build time, which the upstream `pkgs.writers.writePython3` path did not.
      replaceWorkspaceValues = writePythonApplication {
        name = "replace-workspace-values";
        src = ./replace-workspace-values.py;
        pyChecker = "zuban";
        # tomli / tomli-w ship type stubs; the default `extraPaths` points
        # the strict checker at this env's site-packages so the imports
        # resolve under `zuban --strict` (MYPYPATH in writers.nix).
        python = pkgs.python314.withPackages (
          ps:
            attrValues {
              inherit (ps) tomli tomli-w;
            }
        );
      };

      registryPackageSource = pkg: let
        crateTarball = pkgs.fetchurl {
          name = "crate-${pkg.name}-${pkg.version}.tar.gz";
          url = (getAttr pkg.source registryDownloadUrls) pkg;
          # Cargo verifies `.cargo-checksum.json` against the hex digest from
          # Cargo.lock, and that file is filled from `crateTarball.outputHash`
          # below. Switching to `hash = <SRI>` would make `outputHash` an SRI
          # string and break cargo's check, so the registry tarball stays on
          # the hex-valued `sha256` attr.
          # astlog-ignore: prefer-sri-hash
          sha256 = pkg.checksum;
        };
      in
        assert lib.assertMsg (
          pkg ? checksum
        ) "Package ${pkg.name} ${pkg.version} is missing a Cargo.lock checksum.";
          pkgs.runCommand "${pkg.name}-${pkg.version}" {} ''
            mkdir "$out"
            tar xf ${crateTarball} -C "$out" --strip-components=1
            printf '{"files":{},"package":"${crateTarball.outputHash}"}' > "$out/.cargo-checksum.json"
          '';

      gitPackageSource = pkg: let
        git = parseGitSource pkg.source;

        gitHash =
          checkedOutputHashes.${
            pkg.source
          } or (throw ''
            No hash was found while vendoring the git dependency ${pkg.name}-${pkg.version}.
            Add outputHashes."${pkg.source}".
          '');
        tree =
          sourceOverrides.${
            pkg.source
          } or (pkgs.fetchgit {
            inherit (git) url;
            rev = git.sha;
            hash = gitHash;
            nativeBuildInputs = lib.optional (lib.hasPrefix "ssh://" git.url) pkgs.openssh;
          });
      in
        pkgs.runCommand "${pkg.name}-${pkg.version}"
        {
          nativeBuildInputs = [
            pkgs.cargo
            pkgs.jq
          ];
        }
        ''
          tree=${tree}
          crateCargoTOML=""

          if [ -f "$tree/Cargo.toml" ]; then
            crateCargoTOML=$(cargo metadata --format-version 1 --no-deps --manifest-path "$tree/Cargo.toml" | \
              jq -r '.packages[] | select(.name == "${pkg.name}") | .manifest_path' || :)
          fi

          if [ -z "$crateCargoTOML" ]; then
            while IFS= read -r manifest; do
              crateCargoTOML=$(cargo metadata --format-version 1 --no-deps --manifest-path "$manifest" | \
                jq -r '.packages[] | select(.name == "${pkg.name}") | .manifest_path' || :)
              [ -n "$crateCargoTOML" ] && break
            done < <(find "$tree" -name Cargo.toml)
          fi

          if [ -z "$crateCargoTOML" ]; then
            echo "Cannot find ${pkg.name}-${pkg.version} in ${pkg.source}" >&2
            exit 1
          fi

          crateRoot=$(dirname "$crateCargoTOML")
          cp -prvL "$crateRoot" "$out" || echo "Warning: certain files could not be copied" >&2
          chmod -R u+w "$out"

          if grep -q workspace "$out/Cargo.toml"; then
            ${lib.getExe replaceWorkspaceValues} "$out/Cargo.toml" "$(cargo metadata --format-version 1 --no-deps --manifest-path "$crateCargoTOML" | jq -r .workspace_root)/Cargo.toml"
          fi

          printf '{"files":{},"package":null}' > "$out/.cargo-checksum.json"
        '';

      # Every package here carries a `source` (`dependencyPackages` filtered on
      # it), so map each straight to its vendored package directory.
      packageSource = pkg: {
        name = packageSourceKey pkg;
        value =
          if hasAttr pkg.source registryDownloadUrls
          then registryPackageSource pkg
          else if hasGitSource pkg
          then gitPackageSource pkg
          else throw "Cannot create a package-shaped vendor source for ${pkg.name}-${pkg.version} from ${pkg.source}";
      };
    in
      deepSeq checkedOutputHashes (lib.genAttrs' packages packageSource);

    # The aggregate `linkFarm` vendor dir, one symlink per dependency package
    # pointing at its `vendorSources` entry.
    vendorDir = let
      keyFn = pkg: "${pkg.name}-${pkg.version}";

      duplicateNameVersions = findDuplicatesBy keyFn (gitPackages cargoLock);

      mkVendorEntry = pkg: {
        name = "${pkg.name}-${pkg.version}";
        path = vendorSources.${packageSourceKey pkg};
      };

      vendorEntries = map mkVendorEntry packages;
    in
      assert lib.assertMsg (duplicateNameVersions == []) ''
        Cargo.lock contains multiple git dependencies with the same name-version: ${join ", " duplicateNameVersions}
        cargo-unit cannot generate an aggregate vendor dir for this lock without losing source identity.
      '';
        evalTimeSubstitutable (pkgs.linkFarm "cargo-vendor-dir" vendorEntries);
  in {
    inherit vendorSources vendorDir;
  };
in {
  inherit
    cargoLockFile
    vendorConfigScript
    mkVendor
    ;
}
