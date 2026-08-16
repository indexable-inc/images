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
  # Flips `allowSubstitutes` back on for darwin cross-lane eval-time IFD nodes;
  # threaded through to vendor.nix. See its doc comment.
  evalTimeSubstitutable,
}: let
  inherit (builtins) baseNameOf toString;

  vendorLib = import ./vendor.nix {
    inherit
      lib
      pkgs
      writePythonApplication
      lists
      evalTimeSubstitutable
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
      # Guarded, not just assembled: `compiler.*` renders nightly-only `-Z`
      # flags, and a stable rustc does not skip a flag it has never heard of --
      # it exits 1 with "the option `Z` is only accepted on the nightly
      # compiler" before compiling anything. `embedMetadata`'s default flipped
      # to false in 0529f574fc21 and broke three graphs that pass a stable
      # toolchain to `buildWorkspace` (ix2nix-wasm and the two cross graphs),
      # because nothing paired the flag with the channel (ENG-12992). Reads
      # `ixRustChannel`, so a toolchain from outside
      # `ix.languages.rust.toolchain` carries no channel and is NOT checked;
      # widening that means teaching the other constructors to record one.
      renderFlags = let
        nightlyOnly = lib.optional (!policy.compiler.embedMetadata) "-Zembed-metadata=no";
        channel = rustToolchain'.ixRustChannel or null;
        # The rmeta stability trio exists only in the ix rustc fork
        # (packages/rustc-ix), which records its fork rev on the toolchain it
        # returns (passthru.forkRev, readable as a field for the same reason
        # ixRustChannel is: a fact about the toolchain, not a store-path-name
        # match). Even the pinned default NIGHTLY hard-errors on these
        # ("unknown unstable option: rmeta-content-svh"), so the channel guard
        # below is not enough; pair them with the fork explicitly.
        #
        # The auto defaults (null / "auto"; the option docs in policy.nix
        # carry the evidence and the trades) resolve HERE, against the
        # graph's actual toolchain: byte-stable rmeta is on for every
        # fork-toolchain graph and off for every other toolchain, with no
        # per-caller restatement. Only an EXPLICIT on value can reach the
        # fork guard below, so auto can never trip it.
        #
        # `stripSpans` rejoins the auto set (the #10170 re-flip path): the
        # rev that miscompiled under it (c9cc9be58818, hygiene erased along
        # with the location because encode_span substituted DUMMY_SP
        # wholesale, so hygiene-distinct same-name module children collided
        # in every reader at build_reduced_graph.rs:420) is behind us. The
        # pinned rev (42daae19928e, fork PR #3) strips only the location
        # half, `DUMMY_SP.with_ctxt(span.ctxt())`, keeping the codec's
        # Partial wire form, so bindings keep their SyntaxContext and
        # convergence is preserved; the fork carries the
        # rmeta-strip-spans-hygiene regression test for both modes on a
        # hygiene-rich crate, and the unbreak was re-proven here on the exact
        # field poison (thiserror's and encoding_rs's .rmeta re-read by their
        # real consumers, and encoding_rs byte-identical across a
        # line-shifting comment edit with the full trio). The cutoff check
        # (cargo-unit-rmeta-cutoff) rides the auto default again, so the
        # DEFAULT path is what it pins.
        isForkToolchain = rustToolchain' ? forkRev;
        rmetaRaw = policy.compiler.rmetaStability;
        rmeta = {
          contentSvh =
            if rmetaRaw.contentSvh == null
            then isForkToolchain
            else rmetaRaw.contentSvh;
          stripSpans =
            if rmetaRaw.stripSpans == "auto"
            then
              (
                if isForkToolchain
                then "all"
                else "off"
              )
            else rmetaRaw.stripSpans;
          normalizeSrcHash =
            if rmetaRaw.normalizeSrcHash == null
            then isForkToolchain
            else rmetaRaw.normalizeSrcHash;
        };
        rmetaFlags =
          lib.optional rmeta.contentSvh "-Zrmeta-content-svh"
          ++ lib.optional (rmeta.stripSpans != "off") "-Zrmeta-strip-spans=${rmeta.stripSpans}"
          ++ lib.optional rmeta.normalizeSrcHash "-Zrmeta-normalize-src-hash";
        flags =
          lib.optional policy.denyUnusedCrateDependencies "--deny-unused-crate-dependencies"
          ++ lib.optional policy.denyPanics "--deny-panics"
          ++ lib.optional (!policy.compiler.embedMetadata) "--no-embed-metadata"
          ++ map (flag: "--rmeta-stability-flag=${flag}") rmetaFlags;
      in
        assert lib.assertMsg (nightlyOnly == [] || channel == null || channel == "nightly")
        ''
          cargo-unit: policy.compiler renders ${lib.concatStringsSep " " nightlyOnly}, which only a nightly rustc accepts, but this graph's toolchain is channel "${toString channel}".
          Set `policy.compiler.embedMetadata = true` on this graph, or give it a nightly toolchain.
        '';
        assert lib.assertMsg (rmetaFlags == [] || rustToolchain' ? forkRev)
        ''
          cargo-unit: policy.compiler.rmetaStability renders ${lib.concatStringsSep " " rmetaFlags}, which only the ix rustc fork accepts, but this graph's toolchain records no fork rev.
          Give the graph the fork toolchain (pkgs.rustc-ix; the workspace lanes' default rustToolchainVariant already does), leave the trio on its auto defaults, or turn compiler.rmetaStability off explicitly.
        ''; flags;
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
