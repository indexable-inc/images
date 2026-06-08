{
  lib,
  pkgs,
  clippyPackage ? pkgs.clippy,
  rustToolchain ? pkgs.symlinkJoin {
    name = "ix-rust-toolchain";
    paths = [
      pkgs.cargo
      pkgs.rustc
    ];
  },
  writePythonApplication,
}:
let
  inherit (builtins)
    attrNames
    attrValues
    baseNameOf
    deepSeq
    elemAt
    filter
    getAttr
    groupBy
    hasAttr
    length
    match
    removeAttrs
    toJSON
    toString
    ;

  inherit (lib)
    any
    join
    subtractLists
    ;

  joinLines = join "\n";

  toFlagSequence =
    flag:
    lib.concatMap (arg: [
      flag
      arg
    ]);

  isEmpty = l: l == [ ];
  nonEmpty = l: l != [ ];

  findDuplicatesBy =
    keyfn: list:
    let
      groups = groupBy keyfn list;
      multiples = lib.filterAttrs (_: group: length group > 1) groups;
    in
    attrNames multiples;

  optionalInherit = s: k: if s ? "${k}" then { "${k}" = s."${k}"; } else { };
  optionalInherits = s: keys: lib.mergeAttrsList (map (optionalInherit s) keys);

  defaultRustToolchain = rustToolchain;

  # A toolchain's id is the basename of its store path. It is baked into every
  # unit hash by the renderer, so cargoUnit derives it for the default toolchain,
  # the render call, the workspace-side injection cross-check, and the prebuilt
  # builder's assertion. One definition here, at the toolchain owner, keeps those
  # four readings from drifting.
  toolchainId = toolchain: baseNameOf (toString toolchain);

  defaultPolicy = {
    denyUnusedCrateDependencies = true;
    # Opt-in: scans each unit's objects for functions that can reach a panic.
    # Off by default because it is a best-effort gate, not a soundness proof.
    denyPanics = false;
    # On by default: the cargo-audit check is an offline, lockfile-only runCommand
    # (`cargo-audit audit --file Cargo.lock --no-fetch --stale` against the
    # pinned advisory DB) that inherits only Cargo.lock, so it is decoupled from
    # compilation and re-runs only when the lockfile or DB changes. Cheap enough
    # to audit every workspace; opt out per-workspace with a named reason (e.g.
    # lib/rust-workspace.nix disables it on the pure-build cross graph).
    cargoAudit = {
      enable = true;
      db = pkgs.fetchFromGitHub {
        owner = "rustsec";
        repo = "advisory-db";
        rev = "f2ae5fc8e5d208373b6c838f9676434525327a72";
        hash = "sha256-iqXYpuCoWoGypnpM5ceXN748QlYeBXDtZx0uI98qFLo=";
      };
      deny = [ ];
      ignore = [ ];
    };
    cargoMachete = {
      enable = true;
      extraArgs = [ ];
    };
    clippy = {
      enable = true;
      package = clippyPackage;
      cargoArgs = [ "--all-targets" ];
      deniedLints = [ ];
      allowedLints = [ ];
    };
    tests = {
      enable = true;
      useNextest = true;
    };
    linker = {
      useMold = pkgs.stdenv.hostPlatform.isLinux;
    };
  };

  cargoLockFile = cargoLock: cargoLock.lockFile or cargoLock;

  # `platform` is a rust target triple (e.g. `x86_64-unknown-linux-gnu`); mold is
  # Linux-only, so the flags are gated on a `-linux-` triple. Host builds pass
  # `pkgs.stdenv.hostPlatform.config` rather than a sentinel, so there is one
  # Linux test and a non-triple argument fails loudly instead of defaulting.
  rustcArgsForPolicyForPlatform =
    policy: platform:
    lib.optionals (policy.linker.useMold && lib.hasInfix "-linux-" platform) [
      "-C"
      "link-arg=-fuse-ld=mold"
    ];

  # Apply the rustflags a normal `cargo build` reads from `.cargo/config.toml`,
  # which cargoUnit otherwise ignores (it assembles rustc args itself instead of
  # going through cargo). Returns the rustc args for a target triple following
  # cargo precedence: `target.<triple>.rustflags` wins outright over
  # `build.rustflags` (cargo does not merge the two). Flags may be a TOML array
  # or a single whitespace-separated string. `cfg(...)` target sections and the
  # `[env]` table are NOT honored. A `configPath` that does not exist yields no
  # flags, so callers may pass the path unconditionally.
  rustflagsFromCargoConfig =
    configPath: platform:
    let
      config = if builtins.pathExists configPath then lib.importTOML configPath else { };
      normalize =
        flags:
        if builtins.isList flags then flags else filter (flag: flag != "") (lib.splitString " " flags);
      targetFlags = config.target.${platform}.rustflags or null;
      buildFlags = config.build.rustflags or null;
      chosen = if targetFlags != null then targetFlags else buildFlags;
    in
    lib.optionals (chosen != null) (normalize chosen);

  nativeBuildInputsForPolicy = policy: lib.optional policy.linker.useMold pkgs.mold;

  dependencyPackages =
    cargoLock:
    let
      lock = lib.importTOML (cargoLockFile cargoLock);
    in
    filter (pkg: pkg ? source) (lock.package or [ ]);

  hasGitSource = pkg: lib.hasPrefix "git+" pkg.source;

  gitPackages = cargoLock: filter hasGitSource (dependencyPackages cargoLock);

  packageSourceKey = pkg: "${pkg.source}#${pkg.name}@${pkg.version}";

  # Both registry shapes resolve to the same CDN artifact. `static.crates.io` is
  # the direct CloudFront URL cargo's sparse protocol uses; the older
  # `api.crates.io/api/v1/crates/.../download` endpoint just 302s here and, as
  # of 2026-05, rejects curl's default User-Agent with HTTP 403.
  registryDownloadUrls =
    let
      cratesIoDownloadUrl =
        pkg: "https://static.crates.io/crates/${pkg.name}/${pkg.name}-${pkg.version}.crate";
    in
    {
      "registry+https://github.com/rust-lang/crates.io-index" = cratesIoDownloadUrl;
      "sparse+https://index.crates.io/" = cratesIoDownloadUrl;
    };

  parseGitSource =
    source:
    let
      parts = match ''git\+([^?]+)(\?(rev|tag|branch)=([^#]*))?#(.*)'' source;
    in
    if parts == null then
      throw "rust: cannot parse git source string `${source}` from Cargo.lock"
    else
      {
        url = elemAt parts 0;
        refType = elemAt parts 2;
        ref = elemAt parts 3;
        sha = elemAt parts 4;
      };

  clippyLintArgs =
    policy:
    toFlagSequence "-D" policy.clippy.deniedLints ++ toFlagSequence "-A" policy.clippy.allowedLints;

  # Cargo only emits `[lints.clippy]` into the unit graph's `lint_rustflags`
  # when invoked as `cargo clippy`, not `cargo build`. Parse the workspace
  # manifest and emit the equivalent `-D|-W|-A clippy::<lint>` flags so
  # per-unit clippy sees the workspace lint policy.
  clippyLintFlagsFromManifest =
    manifestPath:
    let
      # `clippy::cargo` group lints invoke `cargo` to read workspace metadata.
      # Per-unit clippy runs in a sandboxed build directory without a discoverable
      # Cargo.toml (the unit's source closure is package-shaped), so those lints
      # error out with "could not find Cargo.toml". Skip them here; a future
      # workspace-level cargo-clippy check is the right home.
      cargoGroupClippyLints = [
        "cargo"
        "cargo_common_metadata"
        "multiple_crate_versions"
        "negative_feature_names"
        "redundant_feature_names"
        "wildcard_dependencies"
      ];

      manifest = lib.importTOML manifestPath;

      raw = manifest.workspace.lints.clippy or manifest.lints.clippy or { };

      filtered = removeAttrs raw cargoGroupClippyLints;

      entryFor = name: value: {
        inherit name;
        level = value.level or value;
        priority = value.priority or 0;
      };

      entries = lib.mapAttrsToList entryFor filtered;

      sortedEntries = lib.sortOn (v: v.priority) entries;

      levelFlags = {
        deny = "-D";
        forbid = "-D";
        warn = "-W";
        allow = "-A";
      };

      entryFlags =
        entry:
        let
          inherit (entry) level;
          levelFlag =
            levelFlags."${level}"
              or (throw "cargoUnit: unknown clippy lint level '${level}' in ${manifestPath}");
        in
        [
          levelFlag
          "clippy::${entry.name}"
        ];
    in
    lib.concatMap entryFlags sortedEntries;

  vendorConfigScript =
    {
      cargoExtraConfig,
      cargoLock,
      vendorDir,
    }:
    let
      cargoExtraConfigFile = pkgs.writeText "cargo-extra-config.toml" cargoExtraConfig;

      gitSources = lib.unique (
        map (pkg: parseGitSource pkg.source // { inherit (pkg) source; }) (gitPackages cargoLock)
      );

      # The default vendored-sources config, emitted when the vendor dir carries
      # no `.cargo/config.toml` of its own (the aggregate `linkFarm` never does).
      vendorConfigFile = pkgs.writeText "cargo-vendor-config.toml" ''
        [source.crates-io]
        replace-with = "vendored-sources"

        [source.vendored-sources]
        directory = "${vendorDir}"
      '';

      # One `[source."<git>"]` table per git dependency, each replacing that git
      # source with the vendored copy. Rendered at eval time and `cat` in, rather
      # than printf'd table by table in the builder.
      gitSourceBlock =
        git:
        joinLines (
          [
            ''[source."${git.source}"]''
            "git = ${toJSON git.url}"
          ]
          ++ lib.optional (git.refType != null) "${git.refType} = ${toJSON git.ref}"
          ++ [ ''replace-with = "vendored-sources"'' ]
        );
      gitSourceConfigFile = pkgs.writeText "cargo-git-sources.toml" (
        lib.concatMapStringsSep "\n\n" gitSourceBlock gitSources + "\n"
      );
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
    + lib.optionalString (gitSources != [ ]) ''

      printf '\n' >> "$CARGO_HOME/config.toml"
      cat ${gitSourceConfigFile} >> "$CARGO_HOME/config.toml"
    ''
    + lib.optionalString (cargoExtraConfig != "") ''

      printf '\n' >> "$CARGO_HOME/config.toml"
      cat ${cargoExtraConfigFile} >> "$CARGO_HOME/config.toml"
    '';

  # The "run cargo in the vendored tree" context, resolved once at this boundary
  # and shared by every consumer that needs it together: `policyChecksFor`,
  # `buildPackage` here, and cargoUnit's `generateUnitGraph` / `generateUnitsNix` /
  # workspace import. Both files are two pieces of one unit, so the lockfile,
  # toolchain, policy, and vendor resolution live here rather than being re-derived
  # per side. Each entry point normalizes its raw args exactly once and threads the
  # result onward; nothing downstream re-normalizes or re-checks these values.
  #
  # Defaults are applied here and only here, and the policy/vendor resolution that
  # used to live in standalone single-use helpers is inlined as the `policy`,
  # `vendorSources`, and `vendorDir` bindings below. Vendor resolution stays lazy,
  # so lockfile-only consumers never force the vendor derivations.
  #
  # Per-consumer knobs (`pname`, `rustPlatform`, clippy's cargoArgs override, and
  # cargoUnit's `profile` / `contentAddressed` / `test*` / ...) are not here: each
  # has a single reader and is resolved at that use site.
  normalizeArgs =
    args:
    let
      rustToolchain = args.rustToolchain or defaultRustToolchain;
      cargoLock = args.cargoLock or (args.src + "/Cargo.lock");
      outputHashes = args.outputHashes or { };
      sourceOverrides = args.sourceOverrides or { };
      packages = dependencyPackages cargoLock;

      # The caller's partial policy merged over `defaultPolicy`, field by field.
      # Enumerating the fields (rather than a recursive merge) rejects typo'd keys
      # and keeps the one special case visible: `clippy.denyWarnings = false` drops
      # the "warnings" deny so a warning does not fail the build.
      policy =
        let
          rawPolicy = args.policy or { };
          cargoAudit = rawPolicy.cargoAudit or { };
          cargoMachete = rawPolicy.cargoMachete or { };
          clippy = rawPolicy.clippy or { };
          tests = rawPolicy.tests or { };
          linker = rawPolicy.linker or { };
        in
        {
          denyUnusedCrateDependencies =
            rawPolicy.denyUnusedCrateDependencies or defaultPolicy.denyUnusedCrateDependencies;
          denyPanics = rawPolicy.denyPanics or defaultPolicy.denyPanics;
          cargoAudit = {
            enable = cargoAudit.enable or defaultPolicy.cargoAudit.enable;
            db = cargoAudit.db or defaultPolicy.cargoAudit.db;
            deny = cargoAudit.deny or defaultPolicy.cargoAudit.deny;
            ignore = cargoAudit.ignore or defaultPolicy.cargoAudit.ignore;
          };
          cargoMachete = {
            enable = cargoMachete.enable or defaultPolicy.cargoMachete.enable;
            extraArgs = cargoMachete.extraArgs or defaultPolicy.cargoMachete.extraArgs;
          };
          clippy = {
            enable = clippy.enable or defaultPolicy.clippy.enable;
            package = clippy.package or defaultPolicy.clippy.package;
            cargoArgs = clippy.cargoArgs or defaultPolicy.clippy.cargoArgs;
            deniedLints =
              let
                denied = clippy.deniedLints or defaultPolicy.clippy.deniedLints;
              in
              if !(clippy.denyWarnings or true) then filter (lint: lint != "warnings") denied else denied;
            allowedLints = clippy.allowedLints or defaultPolicy.clippy.allowedLints;
          };
          tests = {
            enable = tests.enable or defaultPolicy.tests.enable;
            useNextest = tests.useNextest or defaultPolicy.tests.useNextest;
          };
          linker = {
            useMold = linker.useMold or defaultPolicy.linker.useMold;
          };
        };

      # One package-shaped vendored source directory per dependency, keyed by
      # `packageSourceKey`. Registry crates are fetched from `static.crates.io`;
      # git crates are fetched and reduced to the single referenced package.
      vendorSources =
        let
          checkedOutputHashes =
            let
              gitPackageSources = map (pkg: pkg.source) (gitPackages cargoLock);
              outputHashKeys = attrNames outputHashes;

              missing = subtractLists outputHashKeys gitPackageSources;
              unused = subtractLists gitPackageSources outputHashKeys;
            in
            assert lib.assertMsg (missing == [ ]) ''
              outputHashes is missing hashes for git source strings in Cargo.lock: ${join ", " missing}
              Key each git hash by the exact Cargo.lock source string, for example:
              outputHashes."git+https://github.com/owner/repo#rev" = "sha256-...";
            '';
            assert lib.assertMsg (unused == [ ]) ''
              outputHashes contains keys that are not git source strings in Cargo.lock: ${join ", " unused}
              Key each git hash by the exact Cargo.lock source string, for example:
              outputHashes."git+https://github.com/owner/repo#rev" = "sha256-...";
            '';
            outputHashes;
          # Flatten workspace inheritance in a vendored Cargo.toml before rustc sees it.
          # Vendored from nixpkgs so a downstream rename of
          # `pkgs/build-support/rust/replace-workspace-values.py` doesn't surface as a
          # `readFile` error here; `ix.writePythonApplication` also runs ty on the body
          # at build time, which the upstream `pkgs.writers.writePython3` path did not.
          replaceWorkspaceValues = writePythonApplication {
            name = "replace-workspace-values";
            src = ./replace-workspace-values.py;
            python = pkgs.python314.withPackages (
              ps:
              attrValues {
                inherit (ps) tomli tomli-w;
              }
            );
          };

          registryPackageSource =
            pkg:
            let
              crateTarball = pkgs.fetchurl {
                name = "crate-${pkg.name}-${pkg.version}.tar.gz";
                url = (getAttr pkg.source registryDownloadUrls) pkg;
                # Cargo verifies `.cargo-checksum.json` against the hex digest from
                # Cargo.lock, and that file is filled from `crateTarball.outputHash`
                # below. Switching to `hash = <SRI>` would make `outputHash` an SRI
                # string and break cargo's check, so the registry tarball stays on
                # the hex-valued `sha256` attr.
                # ast-grep-ignore: prefer-sri-hash
                sha256 = pkg.checksum;
              };
            in
            assert lib.assertMsg (
              pkg ? checksum
            ) "Package ${pkg.name} ${pkg.version} is missing a Cargo.lock checksum.";
            pkgs.runCommand "${pkg.name}-${pkg.version}" { } ''
              mkdir "$out"
              tar xf ${crateTarball} -C "$out" --strip-components=1
              printf '{"files":{},"package":"${crateTarball.outputHash}"}' > "$out/.cargo-checksum.json"
            '';

          gitPackageSource =
            pkg:
            let
              git = parseGitSource pkg.source;

              gitHash =
                checkedOutputHashes.${pkg.source} or (throw ''
                  No hash was found while vendoring the git dependency ${pkg.name}-${pkg.version}.
                  Add outputHashes."${pkg.source}".
                '');
              tree =
                sourceOverrides.${pkg.source} or (pkgs.fetchgit {
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
                  pkgs.jaq
                ];
              }
              ''
                tree=${tree}
                crateCargoTOML=""

                if [ -f "$tree/Cargo.toml" ]; then
                  crateCargoTOML=$(cargo metadata --format-version 1 --no-deps --manifest-path "$tree/Cargo.toml" | \
                    jaq -r '.packages[] | select(.name == "${pkg.name}") | .manifest_path' || :)
                fi

                if [ -z "$crateCargoTOML" ]; then
                  while IFS= read -r manifest; do
                    crateCargoTOML=$(cargo metadata --format-version 1 --no-deps --manifest-path "$manifest" | \
                      jaq -r '.packages[] | select(.name == "${pkg.name}") | .manifest_path' || :)
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
                  ${lib.getExe replaceWorkspaceValues} "$out/Cargo.toml" "$(cargo metadata --format-version 1 --no-deps --manifest-path "$crateCargoTOML" | jaq -r .workspace_root)/Cargo.toml"
                fi

                printf '{"files":{},"package":null}' > "$out/.cargo-checksum.json"
              '';

          # Every package here carries a `source` (`dependencyPackages` filtered on
          # it), so map each straight to its vendored package directory.
          packageSource = pkg: {
            name = packageSourceKey pkg;
            value =
              if hasAttr pkg.source registryDownloadUrls then
                registryPackageSource pkg
              else if hasGitSource pkg then
                gitPackageSource pkg
              else
                throw "Cannot create a package-shaped vendor source for ${pkg.name}-${pkg.version} from ${pkg.source}";
          };

        in
        deepSeq checkedOutputHashes (lib.genAttrs' packages packageSource);

      # The aggregate `linkFarm` vendor dir, one symlink per dependency package
      # pointing at its `vendorSources` entry.
      vendorDir =
        let
          keyFn = pkg: "${pkg.name}-${pkg.version}";

          duplicateNameVersions = findDuplicatesBy keyFn (gitPackages cargoLock);

          mkVendorEntry = pkg: {
            name = "${pkg.name}-${pkg.version}";
            path = vendorSources.${packageSourceKey pkg};
          };

          vendorEntries = map mkVendorEntry packages;
        in
        assert lib.assertMsg (isEmpty duplicateNameVersions) ''
          Cargo.lock contains multiple git dependencies with the same name-version: ${join ", " duplicateNameVersions}
          cargo-unit cannot generate an aggregate vendor dir for this lock without losing source identity.
        '';
        pkgs.linkFarm "cargo-vendor-dir" vendorEntries;
    in
    {
      inherit (args) src;
      inherit rustToolchain cargoLock;
      cargoArgs = args.cargoArgs or [ "--workspace" ];
      nativeBuildInputs = args.nativeBuildInputs or [ ];
      env = args.env or { };
      cargoExtraConfig = args.cargoExtraConfig or "";
      inherit policy vendorDir vendorSources;
    };

  # Gate each policy check on its flag. Takes the already-normalized args and the
  # crate name from whichever owner normalized them, so this never re-normalizes.
  # clippy also needs to know whether the caller set `clippy.cargoArgs` (a fact
  # the policy merge in `normalizeArgs` flattens away), so the owner threads it
  # through. The three check derivations are built inline (each is referenced once,
  # here) and stay lazy: a disabled check is never forced.
  policyChecksFor =
    {
      args,
      pname,
      clippyCargoArgsSet ? false,
    }:
    let
      configScript = vendorConfigScript {
        inherit (args) cargoExtraConfig cargoLock vendorDir;
      };

      cargoAuditCheck =
        let
          inherit (args.policy) cargoAudit;
          lockFile = cargoLockFile args.cargoLock;

          auditFlags = toFlagSequence "--deny" cargoAudit.deny ++ toFlagSequence "--ignore" cargoAudit.ignore;
        in
        pkgs.runCommand "${pname}-cargo-audit"
          {
            nativeBuildInputs = [ pkgs.cargo-audit ];
            # Stage the lockfile through a derivation input so its store path
            # is realized in every builder's sandbox, not just the one that
            # evaluated the expression.
            inherit lockFile;
          }
          ''
            export CARGO_HOME="$TMPDIR/cargo-home"
            mkdir -p "$CARGO_HOME"
            cp "$lockFile" "$TMPDIR/Cargo.lock"
            cd "$TMPDIR"

            cargo-audit audit \
              --file Cargo.lock \
              --db ${lib.escapeShellArg cargoAudit.db} \
              --no-fetch \
              --stale \
              ${lib.escapeShellArgs auditFlags}

            mkdir -p "$out"
          '';

      cargoMacheteCheck =
        pkgs.runCommand "${pname}-cargo-machete"
          (
            {
              nativeBuildInputs = [
                args.rustToolchain
                pkgs.cacert
                pkgs.cargo-machete
              ]
              ++ args.nativeBuildInputs;
              SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
              CARGO_NET_OFFLINE = "true";
            }
            // args.env
          )
          ''
            ${configScript}

            cd ${args.src}

            cargo-machete \
              --with-metadata --skip-target-dir \
              ${lib.escapeShellArgs args.policy.cargoMachete.extraArgs} \
              .

            mkdir -p "$out"
          '';
      cargoClippyCheck =
        let
          # If the caller already picks targets via `cargoArgs` (e.g.
          # `--all-targets`) and didn't override `clippy.cargoArgs`, drop the
          # policy default so we don't double up.
          cargoTargetSelectors = [
            "--all-targets"
            "--lib"
            "--bin"
            "--bins"
            "--example"
            "--examples"
            "--test"
            "--tests"
            "--bench"
            "--benches"
          ];

          lacksTarget = lib.mutuallyExclusive args.cargoArgs cargoTargetSelectors;

          hasLintPolicy = any nonEmpty [
            args.policy.clippy.deniedLints
            args.policy.clippy.allowedLints
          ];

          clippyArgs =
            args.cargoArgs
            ++ lib.optionals (lacksTarget || clippyCargoArgsSet) args.policy.clippy.cargoArgs
            ++ lib.optional hasLintPolicy "--"
            ++ clippyLintArgs args.policy;

          rustFlags = lib.escapeShellArgs (
            rustcArgsForPolicyForPlatform args.policy pkgs.stdenv.hostPlatform.config
          );
        in
        pkgs.runCommand "${pname}-cargo-clippy"
          (
            {
              nativeBuildInputs = [
                args.rustToolchain
                pkgs.cacert
                args.policy.clippy.package
                pkgs.stdenv.cc
              ]
              ++ args.nativeBuildInputs
              ++ nativeBuildInputsForPolicy args.policy;
              SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
            }
            // args.env
          )
          ''
            ${configScript}

            export CARGO_TARGET_DIR="$TMPDIR/cargo-target"

          ''
        + (lib.optionalString (rustFlags != "") /* bash */ ''
          export RUSTFLAGS="''${RUSTFLAGS:+$RUSTFLAGS }${rustFlags}"
        '')
        + /* bash */ ''

          cd ${args.src}

          cargo clippy \
            --frozen --offline \
            ${lib.escapeShellArgs clippyArgs}

          mkdir -p "$out"
        '';
    in
    lib.optionalAttrs args.policy.cargoAudit.enable {
      cargoAudit = cargoAuditCheck;
    }
    // lib.optionalAttrs args.policy.cargoMachete.enable {
      cargoMachete = cargoMacheteCheck;
    }
    // lib.optionalAttrs args.policy.clippy.enable {
      cargoClippy = cargoClippyCheck;
    };

  buildPackage =
    expandedArgs:
    let
      # Every policy check and build derivation needs a crate name for its
      # derivation name (and `meta.mainProgram`). Require it explicitly rather than
      # papering a missing name over with a sentinel that surfaces far downstream.
      crateName = a: a.pname or a.name or (throw "rust.buildPackage: set `pname` (or `name`).");
      # Shortcut: pass `srcRoot = ./.` for a repo-owned crate whose tracked tree
      # is the build closure. Expands to the standard `gitTracked` filter, defaults
      # `meta.mainProgram` to `pname`, and keeps `normalizeArgs`'s `cargoLock`
      # default (`src + "/Cargo.lock"`) intact.
      rawArgs =
        if expandedArgs ? srcRoot then
          let
            inherit (expandedArgs) srcRoot;
            pname = crateName expandedArgs;
          in
          (removeAttrs expandedArgs [ "srcRoot" ])
          // {
            src = lib.fileset.toSource {
              root = srcRoot;
              fileset = lib.fileset.gitTracked srcRoot;
            };
            meta = (expandedArgs.meta or { }) // {
              mainProgram = expandedArgs.meta.mainProgram or pname;
            };
          }
        else
          expandedArgs;
      args = normalizeArgs rawArgs;

      rustPlatform =
        rawArgs.rustPlatform or (pkgs.makeRustPlatform {
          cargo = args.rustToolchain;
          rustc = args.rustToolchain;
        });

      testEnabled = args.policy.tests.enable && (rawArgs.doCheck or true);

      rustcArgs = rustcArgsForPolicyForPlatform args.policy pkgs.stdenv.hostPlatform.config;

      cargoTestFlags =
        (rawArgs.cargoTestFlags or [ ])
        ++ lib.optional (testEnabled && args.policy.tests.useNextest) "--no-tests=pass";
      # Vendor through our own fetcher (`vendorDir` -> `static.crates.io`)
      # instead of letting nixpkgs's `importCargoLock` re-fetch each crate via
      # the legacy `crates.io/api/v1/crates/.../download` URL. The legacy
      # endpoint is now gated on User-Agent (no `curl/...`) and is a redirect
      # to the same CDN anyway, so going direct is both unblocked and faster.
      # Surface the vendor dir as `cargoDeps` (absolute store path); the
      # cargo-setup hook expects `cargoVendorDir` to be in-source, not a
      # `/nix/store` path. User-supplied `cargoHash`, `cargoDeps`, or
      # `cargoVendorDir` still wins. `normalizeArgs` already resolved `vendorDir`
      # (honoring `sourceOverrides`), so reuse it.
      #
      # nixpkgs's `cargoSetupPostPatchHook` diffs `$cargoDeps/Cargo.lock`
      # against the lockfile in the source tree. The vendor dir only emits the
      # per-crate symlinks, so re-attach the lockfile here.
      defaultCargoDeps = pkgs.runCommand "cargo-deps" { } ''
        mkdir -p "$out"
        cp -RL ${args.vendorDir}/. "$out/"
        cp ${cargoLockFile args.cargoLock} "$out/Cargo.lock"
      '';

      hasCargoMeta = rawArgs ? cargoHash || rawArgs ? cargoDeps || rawArgs ? cargoVendorDir;

      buildArgs =
        removeAttrs rawArgs [
          "cargoArgs"
          "cargoExtraConfig"
          "cargoLock"
          "cargoTestFlags"
          "outputHashes"
          "policy"
          "rustPlatform"
          "rustToolchain"
          "sourceOverrides"
          "vendorDir"
        ]
        // {
          nativeBuildInputs = (rawArgs.nativeBuildInputs or [ ]) ++ nativeBuildInputsForPolicy args.policy;
          inherit cargoTestFlags;
          useNextest = testEnabled && args.policy.tests.useNextest;
        }
        // lib.optionalAttrs (!hasCargoMeta) {
          cargoDeps = defaultCargoDeps;
        }
        // lib.optionalAttrs (rustcArgs != [ ]) {
          RUSTFLAGS = (lib.toList (rawArgs.RUSTFLAGS or [ ])) ++ rustcArgs;
        };

      uncheckedPackage = rustPlatform.buildRustPackage buildArgs;

      policyChecks = policyChecksFor {
        inherit args;
        pname = crateName rawArgs;
        # The policy merge in `normalizeArgs` flattens away whether the caller set
        # `clippy.cargoArgs`; the clippy check needs it, so read it off raw args.
        clippyCargoArgsSet = (rawArgs.policy.clippy or { }) ? cargoArgs;
      };
    in
    # The policy-checked wrapper: the same Rust package with the policy checks
    # attached as `passthru.tests` and symlinked under `$out/rust-policy`. Still
    # the same package identity for eval-time callers that inspect it.
    pkgs.symlinkJoin (
      {
        name = "${uncheckedPackage.name}-policy-checked";
        paths = [ uncheckedPackage ];
        inherit (uncheckedPackage) meta;
        passthru = (uncheckedPackage.passthru or { }) // {
          inherit (args) policy;
          unchecked = uncheckedPackage;
          inherit policyChecks;
          tests =
            (uncheckedPackage.passthru.tests or { })
            // policyChecks
            // lib.optionalAttrs testEnabled { package = uncheckedPackage; };
        };
        postBuild =
          let
            linkPolicyCheck = name: check: ''ln -s ${check} "$out/rust-policy/${name}"'';

            linkPolicyChecks = lib.concatMapAttrsStringSep "\n" linkPolicyCheck policyChecks;
          in
          lib.optionalString (policyChecks != { }) ''
            mkdir -p "$out/rust-policy"
            ${linkPolicyChecks}
          '';
      }
      // optionalInherits uncheckedPackage [
        "pname"
        "version"
      ]
    );
in
{
  inherit
    buildPackage
    cargoLockFile
    clippyLintArgs
    clippyLintFlagsFromManifest
    defaultRustToolchain
    nativeBuildInputsForPolicy
    normalizeArgs
    policyChecksFor
    rustcArgsForPolicyForPlatform
    rustflagsFromCargoConfig
    toolchainId
    vendorConfigScript
    ;
}
