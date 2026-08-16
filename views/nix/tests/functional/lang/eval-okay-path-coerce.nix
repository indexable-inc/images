# Every builtin taking a path runs coerceToPath on its argument (via
# realisePath), so a set carrying __toString or outPath is a path -- which is
# how a derivation reaches readFile. toJSON coerces a __toString result the
# same way, with copyToStore off.
#
# coerceToPath does not copy to the store, so the path literals below stay
# source paths and this pair depends on no store. toJSON's own path arm does
# copy, which is why no bare path is handed to it here.
with builtins;

let
  absent = "/eng12669/definitely/absent";
  drv = {
    type = "derivation";
    outPath = absent;
  };
in
[
  (pathExists { outPath = absent; })
  (pathExists { __toString = self: absent; })
  # __toString's result is coerced again, so it may be another set, and
  # outPath is a tail call, so it may be one too.
  (pathExists { __toString = self: { outPath = absent; }; })
  (pathExists {
    outPath = {
      __toString = s: absent;
    };
  })
  (pathExists drv)
  # __toString wins over outPath when a set carries both.
  (pathExists {
    __toString = self: absent;
    outPath = "/eng12669/never/read";
  })
  # A set wrapping a path value: the source tree is what gets read, not a
  # store copy of it, so these names are the checked-in ones.
  (attrNames (readDir {
    outPath = ./dir1;
  }))
  (readFileType { __toString = self: ./dir2; })
  (pathExists { outPath = ./dir1/a.nix; })
  (toJSON { __toString = self: { outPath = "/x"; }; })
  (toJSON { __toString = self: { __toString = s: "/deep"; }; })
]
