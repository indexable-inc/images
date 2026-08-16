# Every builtin that takes its subject with coerceToString accepts what
# cppnix accepts there: a path, which is copied into the store when that
# call site leaves copyToStore on, and a set, through __toString or outPath.
#
# stringLength and substring rejected a path outright until ENG-12854, which
# stopped autoPatchcilHook, dotnet-ef and renode-bin evaluating, because
# lib.strings.hasSuffix computes stringLength of whatever it is handed and
# makeSetupHook hands it a script path.
#
# The path cases are compared against the same coercion spelled as an
# interpolation rather than against a literal, so this pair carries no hash
# and no store directory and stays true wherever it runs.
with builtins;

let

  p = ./lib.nix;
  s = "${p}";

  drv = {
    type = "derivation";
    outPath = "/nix/store/00000000000000000000000000000000-x";
  };

  named = {
    __toString = self: "abcdef";
  };

in
[
  # copyToStore on: the answer is about the store path, not the source path.
  (stringLength p == stringLength s)
  (substring 0 20 p == substring 0 20 s)
  (substring 0 0 p == "")
  # substring keeps the subject's context, and after the copy that context
  # is the store path the file was copied to.
  (getContext (substring 0 0 p) == getContext s)
  (stringLength (unsafeDiscardStringContext p) == stringLength s)

  # copyToStore off: the source path, uncopied.
  (baseNameOf ./dir1/a.nix)
  (dirOf ./dir1/a.nix)
  (toString ./dir1/a.nix == toString ./dir1 + "/a.nix")

  # A set coerces through outPath at every one of them.
  (stringLength drv)
  (substring 11 4 drv)
  (baseNameOf drv)
  (dirOf drv)
  (toString drv)

  # And through __toString, whose result is coerced again.
  (stringLength named)
  (substring 2 3 named)
  (baseNameOf named)
  (dirOf named)

  # throw and abort coerce their message too, so a set is a message.
  (tryEval (throw named)).success
]
