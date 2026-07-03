# The resolution boundary for the Rust build: turn a caller's raw args into one
# resolved bundle, applying every multiply-read default/transform/decision once.
# Owns the toolchain-id rule and the default toolchain, wires the vendoring and
# policy modules together, and re-exposes their surfaces so both build backends
# read from one place.
{
  lib,
  pkgs,
  clippyPackage,
  rustToolchain,
  writePythonApplication,
  lists,
  # Shared pins reader, threaded through to policy.nix (see its arg doc).
  pins,
}: let
  inherit (builtins) baseNameOf toString;

  vendorLib = import ./vendor.nix {
    inherit
      lib
      pkgs
      writePythonApplication
      lists
      ;
  };

  policyLib = import ./policy.nix {
    inherit
      lib
      pkgs
      clippyPackage
      pins
      ;
    inherit (vendorLib) vendorConfigScript cargoLockFile;
  };

  inherit (vendorLib) cargoLockFile vendorConfigScript;
  inherit
    (policyLib)
    clippyLintArgs
    clippyLintFlagsFromManifest
    crateChecks
    nativeBuildInputsForPolicy
    policyPresets
    resolvePolicy
    rustcArgsForPolicyForPlatform
    workspaceChecks
    ;

  defaultRustToolchain = rustToolchain;

  # A toolchain's id is the basename of its store path. It is baked into every
  # unit hash by the renderer, so it is computed once here (in `context`), at the
  # toolchain owner, rather than re-derived at the render call, the workspace-side
  # injection cross-check, and the prebuilt builder.
  toolchainId = toolchain: baseNameOf (toString toolchain);

  # Resolve a caller's raw args into the shared build context and its derived
  # decisions, once. Returns:
  #   context  — the reified "run cargo in the vendored tree" context: the fields
  #              that always travel together (src, toolchain, vendor dir/sources,
  #              env, native inputs, cargo config) plus the lockfile path,
  #              toolchain id, and cargo config script resolved once.
  #   policy   — the resolved quality-gate decisions (typed schema; `linker` is a
  #              sub-field of it).
  #   effects  — policy consequences computed once: rustc args (mold),
  #              native inputs, clippy lint flags, renderer deny-flags.
  #   checks   — the two altitude-appropriate policy-check sets, bound to this
  #              context: `crate` (audit+machete+clippy) and `workspace`
  #              (audit+machete; per-unit clippy runs in the renderer).
  #   cargoArgs / clippyCargoArgsExplicit — facts the policy merge flattens away.
  # The single-reader knobs (profile, target, cargoTargets, ...) are NOT here;
  # each is read at its one use site.
  resolveArgs = args: let
    rustToolchain' = args.rustToolchain or defaultRustToolchain;
    cargoLock = args.cargoLock or (args.src + "/Cargo.lock");
    outputHashes = args.outputHashes or {};
    sourceOverrides = args.sourceOverrides or {};

    policy = resolvePolicy (args.policy or {});

    # Lazy: a lockfile-only consumer never forces these derivations.
    inherit
      (vendorLib.mkVendor {inherit cargoLock outputHashes sourceOverrides;})
      vendorSources
      vendorDir
      ;

    cargoArgs = args.cargoArgs or ["--workspace"];
    nativeBuildInputs = args.nativeBuildInputs or [];
    env = args.env or {};
    cargoExtraConfig = args.cargoExtraConfig or "";

    context = {
      inherit (args) src;
      rustToolchain = rustToolchain';
      inherit
        vendorDir
        vendorSources
        cargoExtraConfig
        nativeBuildInputs
        env
        ;
      cargoLockPath = cargoLockFile cargoLock;
      toolchainId = toolchainId rustToolchain';
      configScript = vendorConfigScript {inherit cargoExtraConfig cargoLock vendorDir;};
    };

    effects = {
      rustcArgsForPlatform = _platform: [];
      linkRustcArgsForPlatform = rustcArgsForPolicyForPlatform policy;
      rustcArgsForHost = rustcArgsForPolicyForPlatform policy pkgs.stdenv.hostPlatform.config;
      linkerNativeInputs = nativeBuildInputsForPolicy policy;
      clippyLintArgs = clippyLintArgs policy;
      renderFlags =
        lib.optional policy.denyUnusedCrateDependencies "--deny-unused-crate-dependencies"
        ++ lib.optional policy.denyPanics "--deny-panics";
    };

    # The flat record the policy-check builders still consume internally.
    checkArgs = {
      inherit (args) src;
      rustToolchain = rustToolchain';
      inherit
        cargoLock
        cargoArgs
        nativeBuildInputs
        env
        cargoExtraConfig
        policy
        vendorDir
        ;
    };

    checks = {
      crate = {
        pname,
        clippyCargoArgsSet ? false,
      }:
        crateChecks {
          args = checkArgs;
          inherit pname clippyCargoArgsSet;
        };
      workspace = pname:
        workspaceChecks {
          args = checkArgs;
          inherit pname;
        };
    };
  in {
    inherit
      context
      policy
      effects
      checks
      cargoArgs
      ;
    clippyCargoArgsExplicit = (args.policy.clippy or {}) ? cargoArgs;
  };
in
  # The `rust` surface consumed by the cargo-unit side: the resolver bundle plus
  # the few helpers that operate outside it (toolchain id/default for prebuilt
  # units, the manifest clippy-lint reader, the policy presets).
  {
    inherit
      resolveArgs
      toolchainId
      defaultRustToolchain
      clippyLintFlagsFromManifest
      policyPresets
      ;
  }
