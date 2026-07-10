# General attrset helpers beyond `nixpkgs.lib`. Keep these pure and free of any
# consumer's formatting opinions (no flag spellings, no value encoders): a
# caller composes them with its own glue. See packages/agent/codex for the canonical
# use (a nested config tree -> `--config dotted.key=value` flags).
{lib}: let
  inherit (builtins) isAttrs;
  inherit (lib) concatMapAttrs isDerivation;

  # A value is a leaf unless it is a plain (non-derivation) attrset. Leaf
  # semantics match deep-merge.nix: lists and derivations are never recursed
  # into, so a derivation's internal attrs (drvPath, outPath, ...) never leak
  # into the flattened keys.
  isLeaf = v: !(isAttrs v) || isDerivation v;
in {
  /**
  Flatten a nested attrset into a flat one keyed by `.`-joined paths to each
  leaf:

  ```
  { a.b = 1; a.c = 2; d = 3; } => { "a.b" = 1; "a.c" = 2; d = 3; }
  ```

  Recurses only into plain attrsets; lists and derivations are leaves, and an
  empty subtree contributes no keys. Compose with `mapAttrsToList` to render
  dotted key/value pairs (e.g. CLI `--config a.b=1` flags or dotted env
  names).

  Precondition: attr names must not contain `.`. A literal-dot name would
  alias a nested path (`{ "a.b" = 1; a.b = 2; }` both flatten to `"a.b"`) and
  the later one silently wins. Ordinary identifiers never trip this.
  */
  flattenToDotted = let
    go = prefix:
      concatMapAttrs (
        name: value: let
          key =
            if prefix == ""
            then name
            else "${prefix}.${name}";
        in
          if isLeaf value
          then {${key} = value;}
          else go key value
      );
  in
    go "";
}
