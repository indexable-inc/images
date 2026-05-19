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
  defaultClippyDeniedLints = [
    "warnings"
    "clippy::all"
    "clippy::pedantic"
    "clippy::nursery"
    "clippy::cargo"
  ];

  defaultClippyAllowedLints = [
    "clippy::multiple_crate_versions"
  ];

  defaultRustToolchain = rustToolchain;

  defaultRustsecAdvisoryDb = pkgs.fetchFromGitHub {
    owner = "rustsec";
    repo = "advisory-db";
    rev = "f2ae5fc8e5d208373b6c838f9676434525327a72";
    hash = "sha256-iqXYpuCoWoGypnpM5ceXN748QlYeBXDtZx0uI98qFLo=";
  };

  defaultPolicy = {
    denyUnusedCrateDependencies = true;
    cargoAudit = {
      enable = false;
      db = defaultRustsecAdvisoryDb;
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
      deniedLints = defaultClippyDeniedLints;
      allowedLints = defaultClippyAllowedLints;
    };
    tests = {
      enable = true;
      useNextest = true;
    };
    linker = {
      useMold = pkgs.stdenv.hostPlatform.isLinux;
    };
  };

  cargoLockFile = cargoLock: if builtins.isAttrs cargoLock then cargoLock.lockFile else cargoLock;

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
          if (clippy ? denyWarnings) && !clippy.denyWarnings then
            builtins.filter (lint: lint != "warnings") denied
          else
            denied;
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

  platformCanUseMold =
    platform:
    if platform == null then pkgs.stdenv.hostPlatform.isLinux else lib.hasInfix "-linux-" platform;

  rustcArgsForPolicy = policy: rustcArgsForPolicyForPlatform policy null;

  rustcArgsForPolicyForPlatform =
    policy: platform:
    lib.optionals (policy.linker.useMold && platformCanUseMold platform) [
      "-C"
      "link-arg=-fuse-ld=mold"
    ];

  rustFlagsStringForPolicy = policy: lib.concatStringsSep " " (rustcArgsForPolicy policy);

  nativeBuildInputsForPolicy = policy: lib.optionals policy.linker.useMold [ pkgs.mold ];

  cargoLockPackages =
    cargoLock: (builtins.fromTOML (builtins.readFile (cargoLockFile cargoLock))).package or [ ];

  dependencyPackages = cargoLock: builtins.filter (pkg: pkg ? source) (cargoLockPackages cargoLock);

  gitPackages =
    cargoLock: builtins.filter (pkg: lib.hasPrefix "git+" pkg.source) (dependencyPackages cargoLock);

  packageSourceKey = pkg: "${pkg.source}#${pkg.name}@${pkg.version}";

  duplicateGitNameVersions =
    cargoLock:
    let
      packagesByNameVersion = builtins.groupBy (pkg: "${pkg.name}-${pkg.version}") (
        gitPackages cargoLock
      );
      duplicates = lib.filterAttrs (_: packages: builtins.length packages > 1) packagesByNameVersion;
    in
    builtins.attrNames duplicates;

  checkedGitOutputHashes =
    cargoLock: outputHashes:
    let
      expectedSources = builtins.listToAttrs (
        map (pkg: lib.nameValuePair pkg.source true) (gitPackages cargoLock)
      );
      missing = builtins.filter (name: !(builtins.hasAttr name outputHashes)) (
        builtins.attrNames expectedSources
      );
      unused = builtins.filter (name: !(builtins.hasAttr name expectedSources)) (
        builtins.attrNames outputHashes
      );
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

  gitHashForPackage =
    outputHashes: pkg:
    outputHashes.${pkg.source} or (throw ''
      No hash was found while vendoring the git dependency ${pkg.name}-${pkg.version}.
      Add outputHashes."${pkg.source}".
    '');

  exportRustFlagsScript =
    policy:
    let
      rustFlags = rustFlagsStringForPolicy policy;
    in
    lib.optionalString (rustFlags != "") ''
      export RUSTFLAGS="''${RUSTFLAGS:+$RUSTFLAGS }${rustFlags}"
    '';

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

  hasCargoTargetSelector = cargoArgs: lib.any (arg: builtins.elem arg cargoTargetSelectors) cargoArgs;

  clippyCargoArgs =
    rawArgs: args:
    let
      rawPolicy = rawArgs.policy or { };
      rawClippy = rawPolicy.clippy or { };
    in
    if hasCargoTargetSelector args.cargoArgs && !(rawClippy ? cargoArgs) then
      [ ]
    else
      args.policy.clippy.cargoArgs;

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
        duplicateNameVersions = duplicateGitNameVersions cargoLock;
        vendorEntries = builtins.filter (entry: entry != null) (
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

  registryDownloadUrls = {
    "registry+https://github.com/rust-lang/crates.io-index" =
      pkg: "https://crates.io/api/v1/crates/${pkg.name}/${pkg.version}/download";
    "sparse+https://index.crates.io/" =
      pkg: "https://static.crates.io/crates/${pkg.name}/${pkg.name}-${pkg.version}.crate";
  };

  parseGitSource =
    source:
    let
      parts = builtins.match ''git\+([^?]+)(\?(rev|tag|branch)=([^#]*))?#(.*)'' source;
    in
    if parts == null then
      null
    else
      {
        url = builtins.elemAt parts 0;
        refType = builtins.elemAt parts 2;
        ref = builtins.elemAt parts 3;
        sha = builtins.elemAt parts 4;
      };

  # Flatten workspace inheritance in a vendored Cargo.toml before rustc sees it.
  # Vendored from nixpkgs so a downstream rename of
  # `pkgs/build-support/rust/replace-workspace-values.py` doesn't surface as a
  # `readFile` error here; `ix.writePythonApplication` also runs basedpyright on
  # the body at build time, which the upstream `pkgs.writers.writePython3` path
  # did not.
  replaceWorkspaceValues = writePythonApplication {
    name = "replace-workspace-values";
    src = ./rust-replace-workspace-values.py;
    python = pkgs.python314.withPackages (
      ps:
      builtins.attrValues {
        inherit (ps) tomli tomli-w;
      }
    );
  };

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
        checkedOutputHashes = checkedGitOutputHashes cargoLock outputHashes;
        registryPackageSource =
          pkg: source: checksum:
          let
            crateTarball = pkgs.fetchurl {
              name = "crate-${pkg.name}-${pkg.version}.tar.gz";
              url = (builtins.getAttr source registryDownloadUrls) pkg;
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
            tree =
              sourceOverrides.${pkg.source} or (pkgs.fetchgit {
                inherit (git) url;
                rev = git.sha;
                sha256 = gitHashForPackage checkedOutputHashes pkg;
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
        packageSource =
          pkg:
          let
            source = pkg.source or null;
            checksum = pkg.checksum or null;
          in
          if source == null then
            null
          else if builtins.hasAttr source registryDownloadUrls then
            assert lib.assertMsg (checksum != null) ''
              Package ${pkg.name} ${pkg.version} is missing a Cargo.lock checksum.
            '';
            lib.nameValuePair (packageSourceKey pkg) (registryPackageSource pkg source checksum)
          else if lib.hasPrefix "git+" source then
            lib.nameValuePair (packageSourceKey pkg) (gitPackageSource pkg)
          else
            throw "Cannot create a package-shaped vendor source for ${pkg.name}-${pkg.version} from ${source}";
      in
      builtins.deepSeq checkedOutputHashes (
        builtins.listToAttrs (builtins.filter (entry: entry != null) (map packageSource packages))
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
        printf '%s\n' ${lib.escapeShellArg "git = ${builtins.toJSON git.url}"}
        ${lib.optionalString (git.refType != null) ''
          printf '%s\n' ${lib.escapeShellArg "${git.refType} = ${builtins.toJSON git.ref}"}
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

  commonArgs =
    args:
    let
      rustToolchain = args.rustToolchain or defaultRustToolchain;
    in
    {
      inherit (args) src;
      inherit rustToolchain;
      pname = args.pname or args.name or "rust-package";
      cargoLock = args.cargoLock or (args.src + "/Cargo.lock");
      cargoArgs = args.cargoArgs or [ "--workspace" ];
      rustPlatform =
        args.rustPlatform or (pkgs.makeRustPlatform {
          cargo = rustToolchain;
          rustc = rustToolchain;
        });
      nativeBuildInputs = args.nativeBuildInputs or [ ];
      env = args.env or { };
      cargoExtraConfig = args.cargoExtraConfig or "";
      vendorDir = args.vendorDir or null;
      outputHashes = args.outputHashes or { };
      policy = resolvePolicy (args.policy or { });
    };

  policyCheckArgs =
    rawArgs:
    let
      args = commonArgs rawArgs;
      vendorDir = resolveVendorDir {
        inherit (args) cargoLock outputHashes vendorDir;
      };
    in
    args // { inherit vendorDir; };

  mkdirOut = ''
    mkdir -p "$out"
  '';

  linkPolicyChecks =
    policyChecks:
    lib.concatStringsSep "\n" (
      lib.mapAttrsToList (name: check: "ln -s ${check} \"$out/rust-policy/${name}\"") policyChecks
    );

  cargoAuditCheck =
    rawArgs:
    let
      args = commonArgs rawArgs;
      inherit (args.policy) cargoAudit;
      auditFlags = [
        "audit"
        "--file"
        (builtins.toString (cargoLockFile args.cargoLock))
        "--db"
        (builtins.toString cargoAudit.db)
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
    pkgs.runCommand "${args.pname}-cargo-audit"
      {
        nativeBuildInputs = [ pkgs.cargo-audit ];
      }
      ''
        export CARGO_HOME="$TMPDIR/cargo-home"
        mkdir -p "$CARGO_HOME"
        cargo-audit ${lib.escapeShellArgs auditFlags}
        ${mkdirOut}
      '';

  cargoMacheteCheck =
    rawArgs:
    let
      args = policyCheckArgs rawArgs;
      macheteArgs = [
        "--with-metadata"
        "--skip-target-dir"
      ]
      ++ args.policy.cargoMachete.extraArgs
      ++ [ "." ];
    in
    pkgs.runCommand "${args.pname}-cargo-machete"
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
    rawArgs:
    let
      args = policyCheckArgs rawArgs;
      clippyArgs = [
        "clippy"
        "--frozen"
        "--offline"
      ]
      ++ args.cargoArgs
      ++ clippyCargoArgs rawArgs args
      ++ lib.optionals (args.policy.clippy.deniedLints != [ ] || args.policy.clippy.allowedLints != [ ]) [
        "--"
      ]
      ++ clippyLintArgs args.policy;
    in
    pkgs.runCommand "${args.pname}-cargo-clippy"
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
        ${exportRustFlagsScript args.policy}
        cd ${args.src}
        cargo ${lib.escapeShellArgs clippyArgs}
        ${mkdirOut}
      '';

  policyChecksFor =
    rawArgs:
    let
      args = commonArgs rawArgs;
    in
    lib.optionalAttrs args.policy.cargoAudit.enable {
      cargoAudit = cargoAuditCheck rawArgs;
    }
    // lib.optionalAttrs args.policy.cargoMachete.enable {
      cargoMachete = cargoMacheteCheck rawArgs;
    }
    // lib.optionalAttrs args.policy.clippy.enable {
      cargoClippy = cargoClippyCheck rawArgs;
    };

  withPolicyChecks =
    {
      package,
      policyChecks,
      extraTests ? { },
      extraPassthru ? { },
    }:
    pkgs.symlinkJoin {
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
        ${linkPolicyChecks policyChecks}
      '';
    };

  buildPackage =
    rawArgs:
    let
      args = commonArgs rawArgs;
      testEnabled = args.policy.tests.enable && (rawArgs.doCheck or true);
      rustcArgs = rustcArgsForPolicy args.policy;
      cargoTestFlags =
        (rawArgs.cargoTestFlags or [ ])
        ++ lib.optionals (testEnabled && args.policy.tests.useNextest) [ "--no-tests=pass" ];
      buildArgs =
        builtins.removeAttrs rawArgs [
          "cargoArgs"
          "cargoExtraConfig"
          "cargoTestFlags"
          "outputHashes"
          "policy"
          "rustPlatform"
          "rustToolchain"
          "vendorDir"
        ]
        //
          lib.optionalAttrs
            (
              !(rawArgs ? cargoLock)
              && !(rawArgs ? cargoHash)
              && !(rawArgs ? cargoDeps)
              && !(rawArgs ? cargoVendorDir)
            )
            {
              cargoLock.lockFile = cargoLockFile args.cargoLock;
            }
        // {
          nativeBuildInputs = (rawArgs.nativeBuildInputs or [ ]) ++ nativeBuildInputsForPolicy args.policy;
          inherit cargoTestFlags;
          useNextest = rawArgs.useNextest or (testEnabled && args.policy.tests.useNextest);
        }
        // lib.optionalAttrs (rustcArgs != [ ]) {
          RUSTFLAGS = (lib.toList (rawArgs.RUSTFLAGS or [ ])) ++ rustcArgs;
        };
      uncheckedPackage = args.rustPlatform.buildRustPackage buildArgs;
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
    cargoClippyCheck
    cargoMacheteCheck
    cargoLockFile
    defaultClippyAllowedLints
    defaultClippyDeniedLints
    defaultPolicy
    defaultRustToolchain
    defaultRustsecAdvisoryDb
    nativeBuildInputsForPolicy
    policyChecksFor
    resolvePolicy
    resolveVendorSources
    resolveVendorDir
    rustcArgsForPolicy
    rustcArgsForPolicyForPlatform
    rustFlagsStringForPolicy
    vendorConfigScript
    withPolicyChecks
    ;
}
