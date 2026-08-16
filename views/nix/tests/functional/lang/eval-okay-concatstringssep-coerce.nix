# concatStringsSep coerces each element with coerceToString, so a set reaches
# __toString or outPath. No paths here: a path element coerces by copying into
# the store, which would make this pair depend on one.
with builtins;

let
  drv = {
    type = "derivation";
    outPath = "/nix/store/00000000000000000000000000000000-x";
  };
  named = {
    __toString = self: "T";
    outPath = "O";
  };
  nested = {
    outPath = {
      outPath = "deep";
    };
  };
in
[
  (concatStringsSep "-" [
    drv
    drv
  ])
  (concatStringsSep "," [ named ])
  (concatStringsSep "" [ nested ])
  (concatStringsSep ", " [
    "a"
    drv
  ])
  (concatStringsSep "" [ ])
]
