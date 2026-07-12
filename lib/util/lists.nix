# List helpers not covered by `nixpkgs.lib`.
{lib}: let
  /**
  The keys for which `keyfn` maps more than one element of `list` to the same
  value, i.e. the duplicate keys. Returned sorted (it is `attrNames` of the
  grouping), which is fine for the assertion messages this feeds. Pair with
  `lib.getAttrs ... (lib.groupBy keyfn list)` when you need the colliding
  elements themselves.
  */
  findDuplicatesBy = keyfn: list:
    lib.attrNames (lib.filterAttrs (_: group: builtins.length group > 1) (lib.groupBy keyfn list));

  /**
  The elements that appear more than once in `list`. Elements must be strings
  (they key a grouping). Returned sorted and de-duplicated.
  */
  findDuplicates = findDuplicatesBy lib.id;

  /**
  Build an attrset from a list by mapping each element to a
  `lib.nameValuePair`-shaped `{ name; value; }` result.
  */
  genAttrs' = xs: f: lib.genAttrs' xs f;
in {
  inherit findDuplicatesBy findDuplicates genAttrs';
}
