# Public Rust SDK build: link the prebuilt, R2-hosted `ix-sdk-wire` rlib WITHOUT
# its source.
#
# End-to-end shape (ENG-2151 / ENG-2154):
#   1. fetch the prebuilt `ix-sdk-wire` rlib + rmeta from public R2 by SRI
#      (fixed-output `pkgs.fetchurl`; no /nix/store path leaks into the URL).
#   2. wrap them as a cargo-unit library unit with `mkPrebuiltLibraryUnit`
#      (#724 seam), recording the toolchain id they were compiled with.
#   3. `buildWorkspace` the public `sdk/rust` workspace, injecting that prebuilt
#      unit over the metadata-faithful `ix-sdk-wire` stub via `extraUnits`. The
#      stub's generated unit key equals the prebuilt's source-independent hash,
#      so the consumer links the prebuilt rlib and never compiles stub source.
#
# Returns the consumer binary plus a `proof` derivation that (a) runs the
# consumer to show it linked + ran the prebuilt rlib, and (b) checks the from-
# source stub lib unit is EXCLUDED from the consumer's build closure.
{
  lib,
  pkgs,
  ix,
}:
let
  inherit (ix) cargoUnit;

  # The prebuilt artifact's coordinates, captured when ix built `ix-sdk-wire`
  # under the `public-rlib` profile and uploaded it to R2 (ENG-2151). The unit
  # `hash` is the source-independent cargo-unit hash; the public SDK workspace
  # must GENERATE this same hash for its `ix-sdk-wire` stub or the injection is
  # rejected by buildWorkspace's C1 assert.
  wireVersion = "0.1.0";
  wireHash = "134ac8c636bf38ee";
  wireToolchainId = "a2djkcczjhr55zfcqhhxabxkhzai2hpa-rust-default-1.98.0-nightly-2026-05-27";
  r2Base = "https://pub-c52bf5a1e3db4628aaf57fe94cb5de10.r2.dev/rlib/ix-sdk-wire/${wireHash}";

  # Fixed-output fetches: the SRI hash is the store-path identity, so the URL
  # carries no secret and substituters can short-circuit. These are the actual
  # compiled artifacts produced in the ix repo, not rebuilt here.
  wireRlib = pkgs.fetchurl {
    url = "${r2Base}/libix_sdk_wire-${wireHash}.rlib";
    hash = "sha256-WxJF0gSJIJhSvF60nPu9F5xdgD7j6TxtKxvyy1DYals=";
  };
  wireRmeta = pkgs.fetchurl {
    url = "${r2Base}/libix_sdk_wire-${wireHash}.rmeta";
    hash = "sha256-9P5fn/5lVyhaQvVAMspfCK1AUF02VsdF/rCvngN1O2o=";
  };

  # Wrap the fetched rlib+rmeta as a cargo-unit library unit. The Cargo lib
  # TARGET name for package `ix-sdk-wire` is `ix_sdk_wire` (renderer underscores
  # it), which is the leading component of both the unit key and the rlib
  # filename. The toolchain id is asserted equal to the workspace toolchain at
  # eval, so a wrong toolchain fails before link.
  prebuiltWireUnit = cargoUnit.mkPrebuiltLibraryUnit {
    name = "ix_sdk_wire";
    version = wireVersion;
    hash = wireHash;
    rlib = wireRlib;
    rmeta = wireRmeta;
    toolchainId = wireToolchainId;
  };

  # The unit key buildWorkspace injects under. Must equal the key the renderer
  # generates for the from-source stub; mismatch => C1 assert fires and lists
  # the real generated keys, which is how we debug a hash divergence.
  wireUnitKey = "ix_sdk_wire-${wireVersion}-${wireHash}";

  src = ./.;

  # Source string for the snafu git fork, keyed exactly as it appears in
  # `Cargo.lock`. snafu and snafu-derive share this one source, so one entry
  # covers both. Tree SRI taken from ix's nix wiring
  # (indexable-inc/ix nix/lib/workspace-cargo-unit.nix) so the resolved tree is
  # byte-identical to ix's.
  outputHashes = {
    "git+https://github.com/shepmaster/snafu.git#1f8e75f56390c421a198871916100c6316d23d4f" =
      "sha256-bz0kOXgdKkID7NUb4RGPIifdx4vnVuvnjucVjYdfvZE=";
  };

  commonArgs = {
    pname = "ix-sdk-rust";
    inherit src outputHashes;
    workspaceRoot = src;
    cargoArgs = [ "--workspace" ];
    # Match the profile the R2 rlib was built under; the profile is folded into
    # the unit hash, so this must equal ix's `public-rlib`.
    profile = "public-rlib";
    # Per-unit clippy / unused-dep / audit gates do not apply to a prebuilt
    # artifact, and the stub is never compiled, so disable them here. (They do
    # NOT affect the unit hash; that is purely metadata + lint_rustflags +
    # profile + deps + toolchain.)
    policy = {
      denyUnusedCrateDependencies = false;
      cargoAudit.enable = false;
      cargoMachete.enable = false;
      clippy.enable = false;
    };
    # exportReferencesGraph (the closure-exclusion proof below) does not support
    # CA derivations; the unit hash is independent of this flag.
    contentAddressed = false;
  };

  # Baseline: build the workspace from source (no injection). Used only to read
  # the generated stub unit key and to prove the stub's from-source unit is what
  # gets EXCLUDED once the prebuilt is injected.
  fromSource = cargoUnit.buildWorkspace commonArgs;

  # The from-source stub lib unit (answer: it should never end up in the
  # injected consumer's closure).
  fromSourceStubUnit = fromSource.units.${wireUnitKey} or null;

  # Injected: the public SDK workspace with the prebuilt R2 rlib injected over
  # the stub unit. This is the real public SDK build.
  injected = cargoUnit.buildWorkspace (
    commonArgs
    // {
      extraUnits = {
        ${wireUnitKey} = prebuiltWireUnit;
      };
      extraLibraries = {
        ix_sdk_wire = prebuiltWireUnit;
      };
    }
  );

  consumer = injected.binaries.ix-sdk-wire-probe or injected.default;
in
{
  inherit
    consumer
    prebuiltWireUnit
    wireUnitKey
    fromSource
    fromSourceStubUnit
    injected
    ;

  # The proof derivation. Build it on the fleet to verify end-to-end.
  proof =
    pkgs.runCommand "ix-sdk-rust-prebuilt-proof"
      {
        nativeBuildInputs = [ pkgs.gnugrep ];
        # Export the consumer's full build-closure reference graph so we can
        # assert the from-source stub lib unit drv is NOT among its inputs.
        exportReferencesGraph = [
          "consumer-graph"
          consumer.drvPath
        ];
      }
      ''
        # (a) The injected unit IS the prebuilt, distinct from the from-source unit.
        echo "prebuilt unit drv : ${prebuiltWireUnit.drvPath}"
        echo "from-source unit  : ${fromSourceStubUnit.drvPath}"
        if [ "${prebuiltWireUnit.drvPath}" = "${fromSourceStubUnit.drvPath}" ]; then
          echo "error: injected unit equals the from-source unit" >&2
          exit 1
        fi

        # The injected workspace's unit map resolves the key to the prebuilt.
        if [ "${injected.units.${wireUnitKey}.drvPath}" != "${prebuiltWireUnit.drvPath}" ]; then
          echo "error: extraUnits did not override the generated unit" >&2
          exit 1
        fi

        # (b) The prebuilt unit's $out matches the library-unit contract.
        test -f ${prebuiltWireUnit}/lib/libix_sdk_wire-${wireHash}.rlib
        test -f ${prebuiltWireUnit}/lib/libix_sdk_wire-${wireHash}.rmeta
        test -f ${prebuiltWireUnit}/nix-support/extern-path
        grep -q '\.rlib$' ${prebuiltWireUnit}/nix-support/extern-path

        # (c) Runtime: the consumer links + runs the prebuilt rlib. The fn it
        # calls lives only in the real crate (the stub has no such item), so a
        # successful run with the expected output proves the prebuilt was linked.
        ${consumer}/bin/ix-sdk-wire-probe > probe.out
        cat probe.out
        grep -q 'ix-sdk-wire linked: normalize(0)=0 normalize(MAX)=0' probe.out

        # (d) Closure exclusion (the source-less proof, mirroring #724 M1): the
        # from-source stub lib unit's drv must NOT appear in the consumer build
        # closure. exportReferencesGraph wrote the closure to ./consumer-graph as
        # alternating "<path>\n<refcount>\n<ref>..." lines; a plain grep for the
        # stub drv path is enough to assert absence.
        echo "asserting from-source stub unit is absent from consumer closure"
        if grep -qF "${fromSourceStubUnit.drvPath}" consumer-graph; then
          echo "error: from-source ix-sdk-wire unit leaked into the consumer closure" >&2
          exit 1
        fi
        # Sanity: the prebuilt unit (or its output) SHOULD be reachable, so the
        # absence above is meaningful and not a path-format mismatch.
        if ! grep -qF "${prebuiltWireUnit.drvPath}" consumer-graph \
           && ! grep -qF "${prebuiltWireUnit.outPath}" consumer-graph; then
          echo "error: prebuilt unit not found in consumer closure; grep may be mismatched" >&2
          exit 1
        fi

        echo "OK: public ix-sdk links the R2-hosted prebuilt ix-sdk-wire rlib with no stub source in its closure"
        mkdir -p "$out"
      '';
}
