{
  ix,
  lib,
  makeWrapper,
  pkgs,
  src,
  rustToolchain ? null,
}:

let
  toolchain =
    if rustToolchain != null then
      rustToolchain
    else
      pkgs.rust-bin.fromRustupToolchainFile (src + "/rust-toolchain.toml");

  rustcLibPathVar =
    if pkgs.stdenv.hostPlatform.isDarwin then "DYLD_LIBRARY_PATH" else "LD_LIBRARY_PATH";
in
ix.buildRustPackage pkgs {
  pname = "llm-clippy";
  version = "0.1.97";

  inherit src;
  rustToolchain = toolchain;
  # Vendor through ix.buildRustPackage's `resolveVendorDir`, which fetches from
  # `static.crates.io`. Upstream indexable-inc/clippy ships no Cargo.lock, so
  # the patch plants one into $sourceRoot for cargo at build time; the same
  # file is what `cargoLock.lockFile` points at for vendoring.
  cargoLock.lockFile = ./Cargo.lock;
  cargoPatches = [ ./cargo-lock.patch ];

  nativeBuildInputs = [ makeWrapper ];
  buildInputs = [
    pkgs.zlib
  ]
  ++ lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
    pkgs.libiconv
  ];
  doCheck = false;
  # llm-clippy IS the clippy that lints every other repo Rust package, so its
  # own clippy check cannot reach for `llmClippyFor` again - it would recurse
  # forever back into this build. Machete and other policy gates stay on.
  policy.clippy.enable = false;

  # This Clippy fork links against rustc_private crates from its Rust toolchain.
  env.RUSTC_BOOTSTRAP = "1";

  postInstall = ''
    for bin in "$out/bin/cargo-clippy" "$out/bin/clippy-driver"; do
      wrapProgram "$bin" \
        --prefix ${rustcLibPathVar} : "${toolchain}/lib"
    done
  '';

  meta = {
    description = "Clippy tuned for LLM-assisted codebases";
    homepage = "https://github.com/indexable-inc/clippy";
    license = [
      lib.licenses.asl20
      lib.licenses.mit
    ];
    mainProgram = "clippy-driver";
  };

  passthru = {
    inherit toolchain;
  };
}
