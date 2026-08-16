{
  busybox,
  seed,
}:

with import ../config.nix;

# A content-addressed derivation. Its output path is derived from the bytes it
# produces, so an unrelated store can produce the very same path -- which is
# what lets the lower layer hold the output while the overlay holds no
# registration for it. That is the state in which registering the realisation
# writes a foreign key to a row that does not exist.
#
# The builder is busybox-sandbox-shell, which carries no applets at all, only
# `sh`. So the output is a plain file written with the `echo` builtin: no
# `mkdir`, no coreutils. The sandbox is required, as it is for every build in
# this directory.
derivation {
  inherit system seed;
  name = "lower-only-realisation";
  builder = busybox;
  args = [
    "sh"
    "-e"
    (builtins.toFile "lower-only-realisation-builder.sh" ''
      echo "$seed" > "$out"
    '')
  ];
  __contentAddressed = true;
  outputHashMode = "recursive";
  outputHashAlgo = "sha256";
}
