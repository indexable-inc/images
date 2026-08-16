{
  # The ix rustc fork (github.com/indexable-inc/rustc, branch `ix`) built from
  # source and assembled into a rust-overlay-shaped toolchain directory, so it
  # can slot into any `cargoUnit.buildWorkspace { rustToolchain = ...; }`
  # unchanged. The fork carries `-Zdump-test-names` (fork PR #1), the missing
  # upstream capability from rust-lang/rust#50297 that lets cargo-unit discover
  # #[test] names without compiling and linking test binaries inside the
  # manifest IFD. x86_64-linux only: that is the system the CI rust graph and
  # the discovery IFD run on, and every extra lane would rebuild a compiler.
  id = "rustc-ix";
  packageSet.systems = ["x86_64-linux"];
  flake.systems = ["x86_64-linux"];
  overlay.systems = ["x86_64-linux"];
}
