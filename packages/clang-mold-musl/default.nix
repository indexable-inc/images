# `clang-mold-musl`: the linker driver `.cargo/config.toml` invokes for the
# `x86_64-unknown-linux-musl` target. Wraps the cross-musl clang (with the right
# musl sysroot, crt files, and libc.a baked in by the nixpkgs cc-wrapper) and
# forces mold as the underlying linker.
#
# rustc invokes the "linker" as a C compiler driver (set
# `link-self-contained=-linker` so rustc does not bundle lld). The driver picks
# up musl crt + libc from the cc-wrapper; `-fuse-ld=mold` swaps the slow default
# linker for mold.
#
# Set IX_LINKER=wild to use the wild linker instead of mold (experimental).
{
  ix,
  pkgsCross,
  mold,
  wild,
}:
let
  writeBashApplication = ix.writeBashApplication ix.pkgs;
in
writeBashApplication {
  name = "clang-mold-musl";
  runtimeInputs = [
    pkgsCross.musl64.buildPackages.clang
    mold
    wild
  ];
  # The linker invocation from rustc includes "-no-pie" which clang warns about
  # as unused; leave -u/-e/-o defaults alone and don't fail on that.
  text = ''
    linker="''${IX_LINKER:-mold}"
    exec x86_64-unknown-linux-musl-clang "-fuse-ld=$linker" "$@"
  '';
  meta.description = "clang driver for the x86_64-unknown-linux-musl Rust target, linking with mold";
}
