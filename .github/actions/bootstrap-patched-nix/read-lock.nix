let
  lock = builtins.fromJSON (builtins.readFile ./lock.json);
  requireString = field: let
    value = lock.${field} or (throw "bootstrap-patched-nix: lock has no `${field}` field");
  in
    if builtins.isString value && value != ""
    then value
    else throw "bootstrap-patched-nix: lock field `${field}` must be a non-empty string";
in {
  outputPath = requireString "outputPath";
  sourceTree = requireString "sourceTree";
  system = requireString "system";
}
