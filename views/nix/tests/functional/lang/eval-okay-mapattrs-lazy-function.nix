# builtins.mapAttrs never forces its function. prim_mapAttrs forces the set
# and builds one unapplied thunk per attribute, so the function is only
# reached when an attribute's value is, and a set may therefore refer to
# itself through one.
#
# The second binding is nixpkgs' idrisPackages shape reduced to its bones:
#
#   { ... } // builtins.mapAttrs self.f { ... }
#
# Forcing the function while the `//` is being evaluated re-enters the `self`
# thunk that is still being built, which reports infinite recursion for an
# expression that has none.
let
  unforced = builtins.mapAttrs (throw "the function is never forced") { a = 1; };

  self = {
    f = name: value: name;
  } // builtins.mapAttrs self.f { b = 2; };
in
[
  (builtins.isAttrs unforced)
  (builtins.attrNames unforced)
  self.b
]
