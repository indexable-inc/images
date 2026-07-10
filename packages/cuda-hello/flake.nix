{
  description = "cuda-hello: a minimal cuda-oxide Rust->PTX kernel";

  # Pinned to the exact cuda-oxide rev used in Cargo.toml so the toolchain
  # (the `cargo oxide` driver, the nightly, LLVM 22 with NVPTX, CUDA 13, and
  # libclang) and the device/host crates always move together. Bump both in
  # lockstep. nixpkgs follows cuda-oxide so there is one shared closure.
  inputs = {
    cuda-oxide.url = "github:NVlabs/cuda-oxide/d22af5f29738fce099ae38262faa7ab59828865f";
    nixpkgs.follows = "cuda-oxide/nixpkgs";
  };

  outputs = {
    cuda-oxide,
    nixpkgs,
    ...
  }: let
    inherit (nixpkgs) lib;
    # cuda-oxide is Linux-only; mirror the systems it builds for.
    systems = [
      "x86_64-linux"
      "aarch64-linux"
    ];
    eachSystem = lib.genAttrs systems;
  in {
    # `cd packages/cuda-hello && nix develop` drops you into cuda-oxide's full
    # CUDA + Rust environment (inherited verbatim, including the host-driver
    # shellHook), then `cargo oxide build` lowers the kernel to PTX and
    # `cargo oxide run` launches it. Building needs no GPU; running does.
    devShells = eachSystem (
      system: let
        pkgs = import nixpkgs {
          inherit system;
          config = {};
          overlays = [];
        };
      in {
        default = pkgs.mkShell {
          inputsFrom = [cuda-oxide.devShells.${system}.default];
        };
      }
    );
  };
}
