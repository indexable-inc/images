# The Rust build policy: the quality/correctness gates applied to a build
# (unused-dep denial, panic-freedom, cargo-audit, cargo-machete, clippy, tests)
# and the linker choice, plus their consequences (rustc args, native inputs,
# lint flags) and the workspace/crate policy-check derivations. Owns the default
# policy and the caller-merge. The check builders run cargo in the vendored tree,
# so the vendor module's `vendorConfigScript` / `cargoLockFile` are threaded in.
{
  lib,
  pkgs,
  clippyPackage,
  vendorConfigScript,
  cargoLockFile,
}:
let
  inherit (builtins)
    filter
    removeAttrs
    ;

  inherit (lib) any;

  toFlagSequence =
    flag:
    lib.concatMap (arg: [
      flag
      arg
    ]);

  nonEmpty = l: l != [ ];

  # The policy schema, declared once as module options so the defaults, the
  # caller-merge, and typo rejection (no `freeformType`, so an unknown key throws)
  # all come from one declaration. `clippy.denyWarnings` is a write-only knob: the
  # resolver post-filters `deniedLints` with it and drops it from the result.
  policyModule = {
    options = {
      denyUnusedCrateDependencies = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Fail a unit whose declared crate dependencies are unused (rustc gate).";
      };
      # Opt-in: scans each unit's objects for functions that can reach a panic.
      # Off by default because it is a best-effort gate, not a soundness proof.
      denyPanics = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Scan each unit's objects for functions that can reach a panic (best-effort).";
      };
      cargoAudit = {
        # On by default: an offline, lockfile-only runCommand (`cargo-audit audit
        # --file Cargo.lock --no-fetch --stale`) decoupled from compilation, so it
        # re-runs only when the lockfile or DB changes. Opt out on pure-build graphs.
        enable = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = "Run the offline, lockfile-only cargo-audit check.";
        };
        db = lib.mkOption {
          type = lib.types.package;
          default = pkgs.fetchFromGitHub {
            owner = "rustsec";
            repo = "advisory-db";
            rev = "f2ae5fc8e5d208373b6c838f9676434525327a72";
            hash = "sha256-iqXYpuCoWoGypnpM5ceXN748QlYeBXDtZx0uI98qFLo=";
          };
          description = "The advisory database cargo-audit checks against.";
        };
        deny = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ ];
          description = "Advisory ids/warning kinds to escalate to errors.";
        };
        ignore = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ ];
          description = "Advisory ids to ignore.";
        };
      };
      cargoMachete = {
        enable = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = "Run cargo-machete to find unused dependencies across the workspace.";
        };
        extraArgs = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ ];
          description = "Extra arguments passed to cargo-machete.";
        };
      };
      clippy = {
        enable = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = "Run clippy (per unit in a workspace, whole-crate otherwise).";
        };
        package = lib.mkOption {
          type = lib.types.package;
          default = clippyPackage;
          description = "The clippy package providing clippy-driver.";
        };
        cargoArgs = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ "--all-targets" ];
          description = "Target-selection args for the whole-crate `cargo clippy`.";
        };
        deniedLints = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ ];
          description = "Lints denied via `-D` (escape hatch; prefer Cargo.toml `[lints]`).";
        };
        allowedLints = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ ];
          description = "Lints allowed via `-A`.";
        };
        denyWarnings = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = "When false, drop `warnings` from deniedLints so a warning does not fail the build.";
        };
      };
      tests = {
        enable = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = "Run the crate's tests as part of the build.";
        };
        useNextest = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = "Use cargo-nextest for parallel test execution.";
        };
      };
      linker = {
        useMold = lib.mkOption {
          type = lib.types.bool;
          default = pkgs.stdenv.hostPlatform.isLinux;
          description = "Link with mold on Linux.";
        };
      };
    };
  };

  # Named partial policies for recurring intents, so callers reference one name
  # instead of re-spelling the same field set. Resolved against the schema like
  # any caller policy. `pureBuild` turns off every gate: for a pure build artifact
  # (a cross graph, a prebuilt-injection graph) where the native graph already
  # ran clippy/audit/machete/unused-dep over the same sources.
  policyPresets = {
    pureBuild = {
      denyUnusedCrateDependencies = false;
      cargoAudit.enable = false;
      cargoMachete.enable = false;
      clippy.enable = false;
    };
  };

  # Resolve a caller's partial policy against the schema: defaults, merge, and
  # typo rejection come from `evalModules`. `denyWarnings` is applied here by
  # post-filtering `deniedLints` (and then dropped, so it carries no effect of its
  # own); `_module` is stripped so the result is a plain policy record matching
  # the historical shape.
  resolvePolicy =
    userPolicy:
    let
      evaluated =
        (lib.evalModules {
          modules = [
            policyModule
            { config = userPolicy; }
          ];
        }).config;
      deniedLints =
        if evaluated.clippy.denyWarnings then
          evaluated.clippy.deniedLints
        else
          filter (lint: lint != "warnings") evaluated.clippy.deniedLints;
    in
    removeAttrs evaluated [ "_module" ]
    // {
      clippy = removeAttrs evaluated.clippy [ "denyWarnings" ] // {
        inherit deniedLints;
      };
    };

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

  nativeBuildInputsForPolicy = policy: lib.optional policy.linker.useMold pkgs.mold;

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

  # The three policy-check derivations for an already-normalized `args` + crate
  # name. clippy also needs to know whether the caller set `clippy.cargoArgs` (a
  # fact the policy merge flattens away), so the owner threads it through. Built
  # lazily and gated by the `crateChecks` / `workspaceChecks` wrappers below, so a
  # check the caller's altitude does not select is never forced.
  checkDerivations =
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

          rustFlags = lib.concatStringsSep " " (
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
          (
            ''
              ${configScript}

              export CARGO_TARGET_DIR="$TMPDIR/cargo-target"

            ''
            + (lib.optionalString (rustFlags != "") /* bash */ ''
              export RUSTFLAGS="''${RUSTFLAGS:+$RUSTFLAGS }"${lib.escapeShellArg rustFlags}
            '')
            + /* bash */ ''

              cd ${args.src}

              cargo clippy \
                --frozen --offline \
                ${lib.escapeShellArgs clippyArgs}

              mkdir -p "$out"
            ''
          );
    in
    {
      inherit cargoAuditCheck cargoMacheteCheck cargoClippyCheck;
    };

  # The per-crate gate set: clippy runs as a whole-crate `cargo clippy`. Each
  # check is gated on its enable flag and stays lazy.
  crateChecks =
    {
      args,
      pname,
      clippyCargoArgsSet ? false,
    }:
    let
      checks = checkDerivations { inherit args pname clippyCargoArgsSet; };
    in
    lib.optionalAttrs args.policy.cargoAudit.enable { cargoAudit = checks.cargoAuditCheck; }
    // lib.optionalAttrs args.policy.cargoMachete.enable { cargoMachete = checks.cargoMacheteCheck; }
    // lib.optionalAttrs args.policy.clippy.enable { cargoClippy = checks.cargoClippyCheck; };

  # The workspace gate set: audit + machete only. A workspace runs clippy per
  # unit in the renderer (`clippyByPackage`), so a whole-workspace `cargo clippy`
  # is deliberately absent here rather than suppressed after the fact.
  workspaceChecks =
    { args, pname }:
    let
      checks = checkDerivations { inherit args pname; };
    in
    lib.optionalAttrs args.policy.cargoAudit.enable { cargoAudit = checks.cargoAuditCheck; }
    // lib.optionalAttrs args.policy.cargoMachete.enable { cargoMachete = checks.cargoMacheteCheck; };
in
{
  inherit
    resolvePolicy
    policyPresets
    rustcArgsForPolicyForPlatform
    nativeBuildInputsForPolicy
    clippyLintArgs
    clippyLintFlagsFromManifest
    crateChecks
    workspaceChecks
    ;
}
