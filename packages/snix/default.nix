{
  lib,
  ix,
  stdenv,
  protobuf,
  pkg-config,
  cmake,
  perl,
  makeWrapper,
  pkgsStatic,
  runCommand,
}:
# snix is the Rust reimplementation of Nix (TVL depot `git.snix.dev/snix/snix`).
# Upstream packages it with crate2nix (`snix/Cargo.nix` + `crate-hashes.json`):
# ~1100 one-derivation-per-crate builds plus feature powersets, with no shared
# incrementality. Here we build the same `default` CLI through index's
# cargo-unit instead, so snix compiles as one Nix derivation per Cargo rustc
# unit with source-scoped, content-addressed inputs — the same engine that
# builds the rest of this repo's Rust tree.
#
# The Cargo workspace lives in the `snix/` subdirectory of the pinned source
# (`snix-src` flake input, surfaced as `ix.snixSrc`).
let
  snixDir = ix.snixSrc + "/snix";

  workspace = ix.cargoUnit.buildWorkspace {
    pname = "snix";
    src = snixDir;
    # Fetched source: the filtered build input and the package-scope checkout
    # root are the same store path (same shape as the real-workspace checks in
    # tests/default.nix).
    workspaceRoot = snixDir;
    cargoLock = snixDir + "/Cargo.lock";
    cargoArgs = ["--workspace"];

    # snix is third-party: build it, do not lint or audit it. Same relaxed
    # posture as `cargoUnitRealWorkspacePolicy` in tests/default.nix.
    policy = {
      denyUnusedCrateDependencies = false;
      cargoAudit.enable = false;
      cargoMachete.enable = false;
      clippy.enable = false;
    };

    # Build-script tooling, applied to every unit:
    #   * protobuf  — prost-build / tonic-build shell out to `protoc`.
    #   * pkg-config — probed by several `*-sys` crates.
    #   * cmake, perl — aws-lc-sys (rustls' default backend) builds its vendored
    #     C with cmake and generates assembly with perl.
    # `cc` is already supplied by cargo-unit.
    nativeBuildInputs = [
      protobuf
      pkg-config
      cmake
      perl
    ];
    env = {
      PROTOC = "${protobuf}/bin/protoc";
      PROTOC_INCLUDE = "${protobuf}/include";
      # snix's proto build scripts resolve `.proto` files as
      # `$PROTO_ROOT/snix/<crate>/protos/...` (build.rs defaults PROTO_ROOT to
      # `../..`, which only resolves in a full checkout). cargo-unit gives each
      # build script a per-crate *scoped* CARGO_MANIFEST_DIR, so point PROTO_ROOT
      # at the whole snix checkout (the repo root that contains `snix/`).
      PROTO_ROOT = "${ix.snixSrc}";
    };

    # snix-build resolves its sandbox shell at rustc time:
    # `const SANDBOX_SHELL: &str = env!("SNIX_BUILD_SANDBOX_SHELL")` in
    # build/src/buildservice/{bwrap,oci}.rs, so the crate does not compile
    # without it and every merge re-built the whole dependency graph up to this
    # unit only to fail (indexable-inc/index#1863). Mirror upstream's crate
    # override (snix/utils.nix): a static busybox `sh` on Linux (copied into
    # the build sandbox, so it must not carry runtime deps), the system shell
    # on darwin. Scoped to the one package so the busybox store path does not
    # perturb every other unit's identity.
    packageBuildEnv.snix-build.SNIX_BUILD_SANDBOX_SHELL =
      if stdenv.hostPlatform.isLinux
      then "${pkgsStatic.busybox}/bin/sh"
      else "/bin/sh";

    # Git dependencies pinned in snix's Cargo.lock, keyed by the exact lock
    # source string. Refresh with `nix flake update snix-src` then rebuild and
    # copy the corrected hashes from the fetchgit mismatch errors.
    outputHashes = {
      "git+https://github.com/flokli/grpc-rust.git?rev=292ed1a6aa8011208f685e9e116ea205d3156256#292ed1a6aa8011208f685e9e116ea205d3156256" = "sha256-tl2Zqbt26+PfNE5TO/7ITH3VXhf3KUpr26rgennfhj4=";
      "git+https://github.com/flokli/hyper.git?rev=554050c2ac5057110dc76a71022d7f6a7a8c9e2e#554050c2ac5057110dc76a71022d7f6a7a8c9e2e" = "sha256-5Jwxx+cafnawCBV+6VS461uL2TGht8k6xPBf2tAhcO0=";
      "git+https://github.com/tvlfyi/wu-manber.git#0d5b22bea136659f7de60b102a7030e0daaa503d" = "sha256-7YIttaQLfFC/32utojh2DyOHVsZiw8ul/z0lvOhAE/4=";
    };
  };

  # The `default` CLI: the base `snix` dispatcher (crate `snix-cli`, bin `snix`)
  # finds each `snix-<subcommand>` binary on `SNIX_LIBEXEC_PATH`, exactly as
  # snix's own cli/make-cli.nix + cli/default-cli.nix assemble it. virtiofs is a
  # Linux-only, non-default feature, so the plain `--workspace` graph omits it
  # and the binary set is identical across platforms.
  subcommands = [
    "snix-build"
    "snix-castore"
    "snix-castore-http"
    "snix-derivation-show"
    "snix-eval"
    "snix-nar-bridge"
    "snix-nix-daemon"
    "snix-store"
  ];
in
  runCommand "snix"
  {
    nativeBuildInputs = [makeWrapper];
    passthru = {inherit workspace;};
    meta = {
      description = "Rust reimplementation of Nix (snix `default` CLI), built via cargo-unit";
      homepage = "https://snix.dev";
      license = lib.licenses.gpl3Only;
      mainProgram = "snix";
      platforms = lib.platforms.unix;
    };
  }
  ''
    mkdir -p "$out/bin" "$out/libexec"
    ${lib.concatMapStringsSep "\n" (
        name: ''ln -s ${workspace.binaries.${name}}/bin/${name} "$out/libexec/${name}"''
      )
      subcommands}
    makeWrapper ${workspace.binaries.snix}/bin/snix "$out/bin/snix" \
      --suffix SNIX_LIBEXEC_PATH : "$out/libexec"
  ''
