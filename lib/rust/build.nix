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
    deepSeq
    elem
    elemAt
    filter
    getAttr
    groupBy
    hasAttr
    isString
    length
    listToAttrs
    match
    removeAttrs
    toJSON
    toString
    ;

  defaultRustToolchain = rustToolchain;

  defaultPolicy = {
    denyUnusedCrateDependencies = true;
    # Opt-in: scans each unit's objects for functions that can reach a panic.
    # Off by default because it is a best-effort gate, not a soundness proof.
    denyPanics = false;
    # On by default: cargoAuditCheck is an offline, lockfile-only runCommand
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

  # Every policy check and build derivation needs a crate name for its
  # derivation name (and `meta.mainProgram`). Require it explicitly rather than
  # papering a missing name over with a sentinel that surfaces far downstream.
  crateName = args: args.pname or args.name or (throw "rust.buildPackage: set `pname` (or `name`).");

  resolvePolicy =
    rawPolicy:
    let
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

  rustcArgsForPolicyForPlatform =
    policy: platform:
    let
      platformIsLinux =
        if platform == null then pkgs.stdenv.hostPlatform.isLinux else lib.hasInfix "-linux-" platform;
    in
    lib.optionals (policy.linker.useMold && platformIsLinux) [
      "-C"
      "link-arg=-fuse-ld=mold"
    ];

  nativeBuildInputsForPolicy = policy: lib.optional policy.linker.useMold pkgs.mold;

  dependencyPackages =
    cargoLock: filter (pkg: pkg ? source) ((lib.importTOML (cargoLockFile cargoLock)).package or [ ]);

  gitPackages =
    cargoLock: filter (pkg: lib.hasPrefix "git+" pkg.source) (dependencyPackages cargoLock);

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
    lib.concatMap (lint: [
      "-D"
      lint
    ]) policy.clippy.deniedLints
    ++ lib.concatMap (lint: [
      "-A"
      lint
    ]) policy.clippy.allowedLints;

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
      entryFor =
        name: value:
        if isString value then
          {
            inherit name;
            level = value;
            priority = 0;
          }
        else
          {
            inherit name;
            inherit (value) level;
            priority = value.priority or 0;
          };
      entries = lib.mapAttrsToList entryFor filtered;
      sorted = lib.sort (left: right: left.priority < right.priority) entries;
      flagFor =
        level:
        if level == "deny" || level == "forbid" then
          "-D"
        else if level == "warn" then
          "-W"
        else if level == "allow" then
          "-A"
        else
          throw "cargoUnit: unknown clippy lint level '${level}' in ${manifestPath}";
    in
    lib.concatMap (entry: [
      (flagFor entry.level)
      "clippy::${entry.name}"
    ]) sorted;

  resolveVendorDir =
    {
      cargoLock,
      outputHashes,
      sourceOverrides ? { },
      vendorDir,
    }:
    if vendorDir != null then
      vendorDir
    else
      let
        packages = dependencyPackages cargoLock;
        sources = resolveVendorSources {
          inherit cargoLock outputHashes sourceOverrides;
        };
        duplicateNameVersions =
          let
            packagesByNameVersion = groupBy (pkg: "${pkg.name}-${pkg.version}") (gitPackages cargoLock);
            duplicates = lib.filterAttrs (_: pkgs': length pkgs' > 1) packagesByNameVersion;
          in
          attrNames duplicates;
        vendorEntries = filter (entry: entry != null) (
          map (
            pkg:
            if !(pkg ? source) then
              null
            else
              {
                name = "${pkg.name}-${pkg.version}";
                path = sources.${packageSourceKey pkg};
              }
          ) packages
        );
      in
      assert lib.assertMsg (duplicateNameVersions == [ ]) ''
        Cargo.lock contains multiple git dependencies with the same name-version: ${lib.concatStringsSep ", " duplicateNameVersions}
        cargo-unit cannot generate an aggregate vendor dir for this lock without losing source identity.
      '';
      pkgs.linkFarm "cargo-vendor-dir" vendorEntries;

  resolveVendorSources =
    {
      cargoLock,
      outputHashes,
      sourceOverrides ? { },
      vendorSources ? null,
    }:
    if vendorSources != null then
      vendorSources
    else
      let
        packages = dependencyPackages cargoLock;
        checkedOutputHashes =
          let
            expectedSources = listToAttrs (
              map (pkg: lib.nameValuePair pkg.source true) (gitPackages cargoLock)
            );
            missing = filter (name: !(hasAttr name outputHashes)) (attrNames expectedSources);
            unused = filter (name: !(hasAttr name expectedSources)) (attrNames outputHashes);
          in
          assert lib.assertMsg (missing == [ ]) ''
            outputHashes is missing hashes for git source strings in Cargo.lock: ${lib.concatStringsSep ", " missing}
            Key each git hash by the exact Cargo.lock source string, for example:
            outputHashes."git+https://github.com/owner/repo#rev" = "sha256-...";
          '';
          assert lib.assertMsg (unused == [ ]) ''
            outputHashes contains keys that are not git source strings in Cargo.lock: ${lib.concatStringsSep ", " unused}
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
          pkg: source: checksum:
          let
            crateTarball = pkgs.fetchurl {
              name = "crate-${pkg.name}-${pkg.version}.tar.gz";
              url = (getAttr source registryDownloadUrls) pkg;
              # Cargo verifies `.cargo-checksum.json` against the hex digest from
              # Cargo.lock, and that file is filled from `crateTarball.outputHash`
              # below. Switching to `hash = <SRI>` would make `outputHash` an SRI
              # string and break cargo's check, so the registry tarball stays on
              # the hex-valued `sha256` attr.
              # ast-grep-ignore: prefer-sri-hash
              sha256 = checksum;
            };
          in
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
        packageSource =
          pkg:
          let
            source = pkg.source or null;
            checksum = pkg.checksum or null;
          in
          if source == null then
            null
          else if hasAttr source registryDownloadUrls then
            assert lib.assertMsg (checksum != null) ''
              Package ${pkg.name} ${pkg.version} is missing a Cargo.lock checksum.
            '';
            lib.nameValuePair (packageSourceKey pkg) (registryPackageSource pkg source checksum)
          else if lib.hasPrefix "git+" source then
            lib.nameValuePair (packageSourceKey pkg) (gitPackageSource pkg)
          else
            throw "Cannot create a package-shaped vendor source for ${pkg.name}-${pkg.version} from ${source}";
      in
      deepSeq checkedOutputHashes (
        listToAttrs (filter (entry: entry != null) (map packageSource packages))
      );

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
      gitSourceConfig = lib.concatMapStringsSep "\n" (git: ''
        printf '\n'
        printf '%s\n' ${lib.escapeShellArg ''[source."${git.source}"]''}
        printf '%s\n' ${lib.escapeShellArg "git = ${toJSON git.url}"}
        ${lib.optionalString (git.refType != null) ''
          printf '%s\n' ${lib.escapeShellArg "${git.refType} = ${toJSON git.ref}"}
        ''}
        printf '%s\n' 'replace-with = "vendored-sources"'
      '') gitSources;
    in
    ''
      export CARGO_HOME="$TMPDIR/cargo-home"
      mkdir -p "$CARGO_HOME"

      if [ -f "${vendorDir}/.cargo/config.toml" ]; then
        sed 's|directory = "cargo-vendor-dir"|directory = "${vendorDir}"|' \
          "${vendorDir}/.cargo/config.toml" > "$CARGO_HOME/config.toml"
      else
        {
          printf '%s\n' '[source.crates-io]'
          printf '%s\n' 'replace-with = "vendored-sources"'
          printf '\n'
          printf '%s\n' '[source.vendored-sources]'
          printf '%s\n' 'directory = "${vendorDir}"'
        } > "$CARGO_HOME/config.toml"
      fi
    ''
    + lib.optionalString (gitSourceConfig != "") ''

      {
        ${gitSourceConfig}
      } >> "$CARGO_HOME/config.toml"
    ''
    + lib.optionalString (cargoExtraConfig != "") ''

      printf '\n' >> "$CARGO_HOME/config.toml"
      cat ${cargoExtraConfigFile} >> "$CARGO_HOME/config.toml"
    '';

  # The "run cargo in the vendored tree" context, resolved once and shared by
  # every consumer that needs it together: the policy checks below, `buildPackage`
  # here, and cargoUnit's `generateUnitGraph` / `generateUnitsNix` / workspace
  # import. Both files are two pieces of one unit, so the lockfile, toolchain,
  # policy, and vendor resolution live here rather than being re-derived per side.
  #
  # Idempotent: every default is `args.x or _`, and the policy / vendor resolvers
  # short-circuit on already-resolved values, so re-normalizing is a no-op.
  # Vendor resolution stays lazy, so lockfile-only consumers never force it.
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
    in
    {
      inherit (args) src;
      inherit rustToolchain cargoLock;
      cargoArgs = args.cargoArgs or [ "--workspace" ];
      nativeBuildInputs = args.nativeBuildInputs or [ ];
      env = args.env or { };
      cargoExtraConfig = args.cargoExtraConfig or "";
      policy = resolvePolicy (args.policy or { });
      vendorDir = resolveVendorDir {
        inherit cargoLock outputHashes sourceOverrides;
        vendorDir = args.vendorDir or null;
      };
      vendorSources = resolveVendorSources {
        inherit cargoLock outputHashes sourceOverrides;
        vendorSources = args.vendorSources or null;
      };
    };

  # `resolvePolicy` flattens the policy, after which "did the caller set
  # clippy.cargoArgs?" is no longer observable. The clippy check needs to know,
  # so it is read off the raw args here.
  callerSetClippyCargoArgs = rawArgs: (rawArgs.policy.clippy or { }) ? cargoArgs;

  mkdirOut = ''
    mkdir -p "$out"
  '';

  cargoAuditCheck =
    { args, pname }:
    let
      inherit (args.policy) cargoAudit;
      lockFile = cargoLockFile args.cargoLock;
      auditFlags = [
        "audit"
        "--file"
        "Cargo.lock"
        "--db"
        (toString cargoAudit.db)
        "--no-fetch"
        "--stale"
      ]
      ++ lib.concatMap (deny: [
        "--deny"
        deny
      ]) cargoAudit.deny
      ++ lib.concatMap (advisory: [
        "--ignore"
        advisory
      ]) cargoAudit.ignore;
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
        cargo-audit ${lib.escapeShellArgs auditFlags}
        ${mkdirOut}
      '';

  cargoMacheteCheck =
    { args, pname }:
    let
      macheteArgs = [
        "--with-metadata"
        "--skip-target-dir"
      ]
      ++ args.policy.cargoMachete.extraArgs
      ++ [ "." ];
    in
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
        ${vendorConfigScript {
          inherit (args) cargoExtraConfig cargoLock vendorDir;
        }}

        cd ${args.src}
        cargo-machete ${lib.escapeShellArgs macheteArgs}
        ${mkdirOut}
      '';

  cargoClippyCheck =
    {
      args,
      pname,
      clippyCargoArgsSet,
    }:
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
      callerHasTargetSelector = lib.any (arg: elem arg cargoTargetSelectors) args.cargoArgs;
      extraClippyCargoArgs =
        if callerHasTargetSelector && !clippyCargoArgsSet then [ ] else args.policy.clippy.cargoArgs;
      clippyArgs = [
        "clippy"
        "--frozen"
        "--offline"
      ]
      ++ args.cargoArgs
      ++ extraClippyCargoArgs
      ++ lib.optional (
        args.policy.clippy.deniedLints != [ ] || args.policy.clippy.allowedLints != [ ]
      ) "--"
      ++ clippyLintArgs args.policy;
      rustFlags = lib.concatStringsSep " " (rustcArgsForPolicyForPlatform args.policy null);
      exportRustFlags = lib.optionalString (rustFlags != "") ''
        export RUSTFLAGS="''${RUSTFLAGS:+$RUSTFLAGS }${rustFlags}"
      '';
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
        ${vendorConfigScript {
          inherit (args) cargoExtraConfig cargoLock vendorDir;
        }}

        export CARGO_TARGET_DIR="$TMPDIR/cargo-target"
        ${exportRustFlags}
        cd ${args.src}
        cargo ${lib.escapeShellArgs clippyArgs}
        ${mkdirOut}
      '';

  # Normalize and name once, then gate each check on its policy flag. All three
  # checks take the shared normalized args plus the pname; clippy also needs to
  # know whether the caller set `clippy.cargoArgs` (lost after `resolvePolicy`).
  policyChecksFor =
    rawArgs:
    let
      args = normalizeArgs rawArgs;
      pname = crateName rawArgs;
    in
    lib.optionalAttrs args.policy.cargoAudit.enable {
      cargoAudit = cargoAuditCheck { inherit args pname; };
    }
    // lib.optionalAttrs args.policy.cargoMachete.enable {
      cargoMachete = cargoMacheteCheck { inherit args pname; };
    }
    // lib.optionalAttrs args.policy.clippy.enable {
      cargoClippy = cargoClippyCheck {
        inherit args pname;
        clippyCargoArgsSet = callerSetClippyCargoArgs rawArgs;
      };
    };

  withPolicyChecks =
    {
      package,
      policyChecks,
      extraTests ? { },
      extraPassthru ? { },
    }:
    pkgs.symlinkJoin (
      {
        name = "${package.name}-policy-checked";
        paths = [ package ];
        inherit (package) meta;
        passthru =
          (package.passthru or { })
          // extraPassthru
          // {
            unchecked = package;
            inherit policyChecks;
            tests = (package.passthru.tests or { }) // policyChecks // extraTests;
          };
        postBuild = lib.optionalString (policyChecks != { }) ''
          mkdir -p "$out/rust-policy"
          ${lib.concatStringsSep "\n" (
            lib.mapAttrsToList (name: check: "ln -s ${check} \"$out/rust-policy/${name}\"") policyChecks
          )}
        '';
      }
      # The policy wrapper is still the same Rust package for eval-time callers
      # that inspect package identity.
      // lib.optionalAttrs (package ? pname) { inherit (package) pname; }
      // lib.optionalAttrs (package ? version) { inherit (package) version; }
    );

  buildPackage =
    expandedArgs:
    let
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
      rustcArgs = rustcArgsForPolicyForPlatform args.policy null;
      cargoTestFlags =
        (rawArgs.cargoTestFlags or [ ])
        ++ lib.optional (testEnabled && args.policy.tests.useNextest) "--no-tests=pass";
      # Vendor through our own fetcher (`resolveVendorDir` -> `static.crates.io`)
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
      # against the lockfile in the source tree. `resolveVendorDir` only
      # emits the per-crate symlinks, so re-attach the lockfile here.
      defaultCargoDeps = pkgs.runCommand "cargo-deps" { } ''
        mkdir -p "$out"
        cp -RL ${args.vendorDir}/. "$out/"
        cp ${cargoLockFile args.cargoLock} "$out/Cargo.lock"
      '';
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
        //
          lib.optionalAttrs (!(rawArgs ? cargoHash) && !(rawArgs ? cargoDeps) && !(rawArgs ? cargoVendorDir))
            {
              cargoDeps = defaultCargoDeps;
            }
        // {
          nativeBuildInputs = (rawArgs.nativeBuildInputs or [ ]) ++ nativeBuildInputsForPolicy args.policy;
          inherit cargoTestFlags;
          useNextest = testEnabled && args.policy.tests.useNextest;
        }
        // lib.optionalAttrs (rustcArgs != [ ]) {
          RUSTFLAGS = (lib.toList (rawArgs.RUSTFLAGS or [ ])) ++ rustcArgs;
        };
      uncheckedPackage = rustPlatform.buildRustPackage buildArgs;
      policyChecks = policyChecksFor rawArgs;
    in
    withPolicyChecks {
      package = uncheckedPackage;
      inherit policyChecks;
      extraPassthru = {
        inherit (args) policy;
      };
      extraTests = lib.optionalAttrs testEnabled {
        package = uncheckedPackage;
      };
    };
in
{
  inherit
    buildPackage
    cargoAuditCheck
    cargoLockFile
    clippyLintArgs
    clippyLintFlagsFromManifest
    defaultRustToolchain
    nativeBuildInputsForPolicy
    normalizeArgs
    policyChecksFor
    resolvePolicy
    resolveVendorSources
    resolveVendorDir
    rustcArgsForPolicyForPlatform
    vendorConfigScript
    ;
}
