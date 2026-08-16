{
  busybox,
  seed,
}:

let
  config = import ../config.nix;
in

# Input-addressed on purpose, and constructed with `derivation` rather than
# `config.nix`'s `mkDerivation` for two reasons: `mkDerivation` turns
# content-addressed under NIX_TESTS_CA_BY_DEFAULT, and the case under test is
# an output path whose name says nothing about its content; and its builder is
# a dynamically linked bash, which cannot run here because the stores these
# tests build in hold nothing but what the test put there, so the loader and
# libc are absent and `exec` fails with ENOENT. The sandbox `busybox` is
# static, which is why every derivation in this directory uses it -- but it is
# built with `sh` and `ash` as its only applets, so the builder gets shell
# builtins and nothing else. Hence a regular file for `$out` rather than a
# directory: `mkdir` does not exist here.
derivation {
  inherit (config) system;
  inherit seed;
  name = "lower-gains-invisible-path";
  builder = busybox;
  args = [
    "sh"
    "-e"
    (builtins.toFile "lower-gains-invisible-path-builder.sh" ''
      echo "$seed" > "$out"
    '')
  ];
}
