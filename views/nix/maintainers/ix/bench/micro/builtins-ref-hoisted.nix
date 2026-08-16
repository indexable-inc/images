# The builtins-ref.nix loop with the reference hoisted out of it: one
# `builtins.stringLength` reference for the whole run instead of 400k.
let
  stringLength = builtins.stringLength;
in
builtins.foldl' (acc: _: acc + stringLength "abcdefgh") 0 (builtins.genList (i: i) 400000)
