{
  stdenv,
  lib,
  mkMesonExecutable,

  nix-store,
  nix-expr,
  nix-main,
  nix-cmd,

  cargo,
  rustc,
  rustPlatform,

  # Configuration Options

  version,

  # Build the in-tree Rust evaluator and link it in, so that `eval-backend =
  # rust` and `eval-backend = shadow` have something to route to. Off by
  # default: it pulls a Rust toolchain and a vendored cargo registry into a
  # build that otherwise needs neither, and it changes this component's source
  # (the crates below join the fileset), so leaving it off keeps the derivation
  # byte-identical to a build that never heard of the Rust evaluator.
  withRustEval ? false,
}:

let
  inherit (lib) fileset;
in

mkMesonExecutable (finalAttrs: {
  pname = "nix";
  inherit version;

  workDir = ./.;
  fileset = fileset.unions (
    [
      ../../nix-meson-build-support
      ./nix-meson-build-support
      ../../.version
      ./.version
      ./meson.build
      ./meson.options

      # Symbolic links to other dirs
      ## exes
      ./doc
      ## dirs
      ./scripts
      ../../scripts
      ./misc
      ../../misc

      # Doc nix files for --help
      ../../doc/manual/generate-manpage.nix
      ../../doc/manual/utils.nix
      ../../doc/manual/generate-settings.nix
      ../../doc/manual/generate-store-info.nix

      # Other files to be included as string literals
      ./nix-channel/unpack-channel.nix
      ./nix-env/buildenv.nix
      ./get-env.sh
      ./help-stores.md
      ../../doc/manual/source/store/types/index.md.in
      ./profiles.md
      ../../doc/manual/source/command-ref/files/profiles.md

      # Files
    ]
    ++ [
      (fileset.fileFilter (file: file.hasExt "cc") ./.)
      (fileset.fileFilter (file: file.hasExt "hh") ./.)
      (fileset.fileFilter (file: file.hasExt "md") ./.)
    ]
    ++ lib.optionals withRustEval [
      # The Rust evaluator's crates sit outside this component's directory --
      # `src/nix/meson.build` reaches for `../../rust` -- so nothing in the
      # fileset above brings them in, and `-Drust-eval=enabled` fails on a
      # missing Cargo.toml rather than on anything to do with the code.
      #
      # The workspace members are named one by one instead of taking
      # `../../rust` wholesale: a developer who has run `cargo build` by hand
      # has a `rust/target` holding hundreds of MiB of artifacts, and a
      # wholesale include would copy it into the derivation's source and make
      # the output hash depend on whether that directory happened to exist.
      ../../rust/Cargo.toml
      ../../rust/Cargo.lock
      ../../rust/nix-eval-rs
      ../../rust/ix-kernel
      ../../rust/nix-eval-driver
      # nix-eval-rs's build script fingerprints the C++ tree's copy of
      # `derivation.nix` (rust/nix-eval-rs/compiler-fingerprint.rs), so the
      # crate cannot even configure without it. It is a `.nix` file, which no
      # filter above matches.
      ../libexpr/primops/derivation.nix
    ]
  );

  buildInputs = [
    nix-store
    nix-expr
    nix-main
    nix-cmd
  ];

  nativeBuildInputs = lib.optionals withRustEval [
    cargo
    rustc
    rustPlatform.cargoSetupHook
  ];

  # Vendored so cargo never reaches the network from inside the sandbox. The
  # lock pins one dependency to a git rev rather than crates.io (a fork of
  # rnix adding `1_000`-style digit separators), and `importCargoLock` cannot
  # derive a fixed-output hash for a git source on its own, so that one is
  # named explicitly.
  cargoDeps =
    if withRustEval then
      rustPlatform.importCargoLock {
        lockFile = ../../rust/Cargo.lock;
        outputHashes = {
          "rnix-0.12.0" = "sha256-CEBnghY4vr+FTR0d7tUkdjrgXgPtws+EA+Ig8aOM904=";
        };
      }
    else
      null;
  cargoRoot = lib.optionalString withRustEval "../../rust";

  mesonFlags = lib.optionals withRustEval [
    (lib.mesonEnable "rust-eval" true)
  ];

  postInstall = lib.optionalString stdenv.hostPlatform.isStatic ''
    mkdir -p $out/nix-support
    echo "file binary-dist $out/bin/nix" >> $out/nix-support/hydra-build-products
  '';

  meta = {
    mainProgram = "nix";
    platforms = lib.platforms.unix ++ lib.platforms.windows;
  };

})
