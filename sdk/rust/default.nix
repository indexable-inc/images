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

  # The exact toolchain the R2 `ix-sdk-wire` rlib was compiled with. The
  # cargo-unit hash folds in the toolchain id (the store-path basename), so the
  # public SDK workspace MUST build with a toolchain that resolves to id
  # `iz0mdcq43pxl3fmxmznc6n38sals6q0x-rust-default-1.98.0-nightly-2026-05-27`,
  # or the generated `ix-sdk-wire` unit hash diverges from the prebuilt's.
  #
  # ix builds via `rust-bin.fromRustupToolchainFile rust-toolchain.toml`; the
  # equivalent is the index rust toolchain helper with ix's exact pin: the same
  # nightly date, the `default` rust-overlay profile, ix's extra components, and
  # ix's extra targets (ix rust-toolchain.toml added aarch64-apple-darwin in
  # ix#4278, which is what moved the toolchain id off `a2dj...`). The binding
  # constraint is that this resolves to wireToolchainId (the eval assert in
  # mkPrebuiltLibraryUnit enforces it); verified on x86_64-linux at index's
  # rust-overlay d286e969 that this yields the `iz0m...` id above.
  rustToolchain = ix.languages.rust.toolchain pkgs {
    channel = "nightly";
    version = "2026-05-27";
    profile = "default";
    components = [
      "rust-src"
      "rust-analyzer"
      "rustc-dev"
      "llvm-tools"
    ];
    targets = [
      "aarch64-apple-darwin"
      "x86_64-unknown-linux-musl"
      "wasm32-unknown-unknown"
    ];
  };

  # The prebuilt artifact's coordinates, captured when ix built `ix-sdk-wire`
  # under the `public-rlib` profile and uploaded it to R2 (ENG-2151). The unit
  # `hash` is the source-independent cargo-unit hash; the public SDK workspace
  # must GENERATE this same hash for its `ix-sdk-wire` stub or the injection is
  # rejected by buildWorkspace's C1 assert.
  wireVersion = "0.1.0";
  wireHash = "4e5d4b3c3884e404";
  wireToolchainId = "iz0mdcq43pxl3fmxmznc6n38sals6q0x-rust-default-1.98.0-nightly-2026-05-27";
  r2Base = "https://pub-559bccbc8be94bed84821cb943b580f3.r2.dev/rlib/ix-sdk-wire/${wireHash}";

  # Fixed-output fetches: the SRI hash is the store-path identity, so the URL
  # carries no secret and substituters can short-circuit. These are the actual
  # compiled artifacts produced in the ix repo, not rebuilt here.
  wireRlib = pkgs.fetchurl {
    url = "${r2Base}/libix_sdk_wire-${wireHash}.rlib";
    hash = "sha256-ShWsIGI6UAjCA/rWgRs9CMJ7kdak0L3Yzvn8Wjgb+X8=";
  };
  wireRmeta = pkgs.fetchurl {
    url = "${r2Base}/libix_sdk_wire-${wireHash}.rmeta";
    hash = "sha256-zz7SV4SgbuGRzh+nbbLXT09S+HWaZRPgd635fMhXT04=";
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
    # Non-default toolchain: thread the same one buildWorkspace uses, or the
    # eval-time assert (and buildWorkspace's C2 cross-check) reject the unit.
    inherit rustToolchain;
  };

  # The unit key buildWorkspace injects under. Must equal the key the renderer
  # generates for the from-source stub; mismatch => C1 assert fires and lists
  # the real generated keys, which is how we debug a hash divergence.
  wireUnitKey = "ix_sdk_wire-${wireVersion}-${wireHash}";

  # One deterministic source derivation shared by graph generation and rendering.
  # A bare `./.` can realize under two different store paths in the two IFD
  # stages, which makes the renderer reject the local member as "outside
  # workspace root". `fs.toSource` pins a single store path for both.
  fs = lib.fileset;
  src = fs.toSource {
    root = ./.;
    fileset = fs.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./crates
      ./vendor
    ];
  };

  # ix built the rlib with an explicit `--target <triple>`, which stamps each
  # LIBRARY unit's `platform` field (folded into the unit hash) with the triple
  # instead of leaving it null (a host-native build). To generate the same
  # `ix_sdk_wire` hash, the public SDK workspace must build with the SAME target.
  # ix's `hostRustTarget` map; this check only runs on x86_64-linux.
  hostRustTarget =
    {
      x86_64-linux = "x86_64-unknown-linux-gnu";
      aarch64-linux = "aarch64-unknown-linux-gnu";
      aarch64-darwin = "aarch64-apple-darwin";
    }
    .${pkgs.stdenv.hostPlatform.system}
      or (throw "sdk/rust: unsupported host platform ${pkgs.stdenv.hostPlatform.system}");

  # Source string for the snafu git fork, keyed exactly as it appears in
  # `Cargo.lock`. snafu and snafu-derive share this one source, so one entry
  # covers both. Tree SRI taken from ix's nix wiring
  # (indexable-inc/ix nix/lib/workspace-cargo-unit.nix) so the resolved tree is
  # byte-identical to ix's.
  outputHashes = {
    "git+https://github.com/shepmaster/snafu.git#ff50133848f39de1b1fd40c74daa9d781fdda544" =
      "sha256-eSNVZr0TxDguSSu9c3L6S7rwqq45NemtmTvxHdiDRgM=";
  };

  commonArgs = {
    pname = "ix-sdk-rust";
    inherit src outputHashes rustToolchain;
    # Match the target ix built the rlib with, so library units carry the same
    # `platform` (hence the same hash) as the R2 artifact.
    target = hostRustTarget;
    # The real checkout root that package scopes are carved from (mirrors the
    # cargo-unit-prebuilt fixture: plain path for workspaceRoot, filtered
    # `toSource` for src).
    workspaceRoot = ./.;
    cargoArgs = [ "--workspace" ];
    # Match the profile the R2 rlib was built under; the profile is folded into
    # the unit hash, so this must equal ix's `public-rlib`.
    profile = "public-rlib";
    # Per-unit clippy / unused-dep / audit gates do not apply to a prebuilt
    # artifact, and the stub is never compiled, so disable them all. (They do
    # NOT affect the unit hash; that is purely metadata + lint_rustflags +
    # profile + deps + toolchain.)
    policy = cargoUnit.policyPresets.pureBuild;
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
