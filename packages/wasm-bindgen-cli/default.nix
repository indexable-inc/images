# Pinned `wasm-bindgen-cli` matching the `wasm-bindgen` Cargo dep.
#
# Schemas are not stable across patch bumps, so the CLI and the Rust crate MUST
# match exactly. Built via nixpkgs's `buildWasmBindgenCli` factory. Consumers
# that build wasm artifacts pin their `wasm-bindgen` crate to this version.
{
  buildWasmBindgenCli,
  fetchCrate,
  rustPlatform,
}:
let
  version = "0.2.123";
  src = fetchCrate {
    pname = "wasm-bindgen-cli";
    inherit version;
    url = "https://static.crates.io/crates/wasm-bindgen-cli/wasm-bindgen-cli-${version}.crate";
    hash = "sha256-ymeAEYsr7OnupWYJWjSeVGvq3+s+zxSNkODbzY62rYs=";
  };
  # `rustPlatform.importCargoLock` materializes the complete cargoDeps shape
  # (per-crate `<name>-<version>` symlinks, `.cargo/config.toml`, and
  # `Cargo.lock`) and vendors registry crates through `static.crates.io`, so the
  # older `crates.io/api/v1/crates/.../download` endpoint (which rejects
  # cargo-vendor's curl User-Agent) is never used. See nixpkgs
  # `pkgs/build-support/rust/import-cargo-lock.nix`.
  cargoDeps = rustPlatform.importCargoLock {
    lockFile = src + "/Cargo.lock";
  };
in
buildWasmBindgenCli {
  inherit version src cargoDeps;
}
