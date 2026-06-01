# Recursive attrset merge with two collision policies. Single sanctioned
# replacement for the hand-rolled deep-merge helpers that used to live in
# `packages.nix`, `discovery.nix`, `minecraft/dimension-type.nix`, and the two
# `lib.recursiveUpdate` escape hatches in `portable-services.nix`. The
# `no-recursive-update` lint rule points here.
#
# Semantics, identical across both variants:
#   * Recurse only when both sides are non-derivation attrsets.
#   * Derivations are leaves: never recursed into.
#   * Lists are leaves: never element-merged.
#   * Keys present on only one side pass through unchanged.
#
# Collision policy is the only knob, expressed as two named functions instead
# of a flag so a call site cannot accidentally combine settings into a fourth,
# untested semantics.
{ lib }:
let
  inherit (lib) concatStringsSep isDerivation;
  inherit (builtins)
    attrNames
    foldl'
    hasAttr
    isAttrs
    ;

  bothMergeable = a: b: isAttrs a && isAttrs b && !(isDerivation a) && !(isDerivation b);

  mergeWith =
    onCollision:
    let
      go =
        path: lhs: rhs:
        foldl' (
          acc: name:
          let
            r = rhs.${name};
          in
          if hasAttr name acc then
            let
              l = acc.${name};
            in
            if bothMergeable l r then
              acc // { ${name} = go (path ++ [ name ]) l r; }
            else
              acc // { ${name} = onCollision (path ++ [ name ]) l r; }
          else
            acc // { ${name} = r; }
        ) lhs (attrNames rhs);
    in
    go [ ];

  strictMerge = mergeWith (
    path: _l: _r:
    throw "ix.deepMerge.strict: leaf collision at `${concatStringsSep "." path}`"
  );

  rhsMerge = mergeWith (
    _path: _l: r:
    r
  );
in
{
  /**
    Recursively merge two attrsets, throwing on a leaf collision. The error
    message names the dotted attr path that collided. Use when two inputs are
    supposed to contribute disjoint subtrees: an accidental overlap is a bug
    in the caller, not a value to silently resolve.
  */
  strict = strictMerge;

  /**
    Recursively merge two attrsets, with rhs winning at any leaf collision.
    Use for layered overrides where the second argument is a deliberate
    override of the first (vanilla defaults plus user fields, generated unit
    plus escape-hatch keys).
  */
  rhs = rhsMerge;

  /**
    Strict deep-merge folded over a list of attrsets. The N-ary shape that
    `discoverModules` needs (each module tree contributes a subset of the
    final attrset; any overlap is a duplicate-output bug).
  */
  strictList = parts: foldl' strictMerge { } parts;
}
