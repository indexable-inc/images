{
  busybox,
  seed,
}:

with import ../config.nix;

# An input-addressed derivation that is deliberately NOT reproducible: it
# records the host uptime, so two builds of it produce different bytes at the
# same store path. That is what lets the test tell "kept the registered path
# info" apart from "recorded our own hash against someone else's content".
#
# It must also stay input-addressed under NIX_TESTS_CA_BY_DEFAULT, which is why
# it does not go through hermetic.nix's mkDerivation: the whole point is the
# case where the output path name is not a statement about content.
#
# The builder is busybox-sandbox-shell, which carries no applets at all -- only
# `sh`. So: no `mkdir` (the output is a plain file), no `date` (/proc/uptime
# via the `read` builtin), and no `sleep` (a builtin spin). Ugly, but it is the
# only vocabulary available inside the sandbox, and the sandbox is required:
# a non-chroot build writes the output at its final path, which never reaches
# the branch under test.
derivation {
  inherit system seed;
  name = "lower-gains-output";
  builder = busybox;
  args = [
    "sh"
    "-e"
    (builtins.toFile "lower-gains-output-builder.sh" ''
      # Tells the driver the builder is live, so it can register this same
      # output in the lower store while we are still running.
      echo "BUILDER_STARTED" >&2
      read -r uptime rest < /proc/uptime
      i=0
      while [ "$i" -lt 8000000 ]; do
        i=$((i + 1))
      done
      echo "$uptime" > "$out"
      echo "$seed" >> "$out"
    '')
  ];
}
