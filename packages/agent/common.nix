# Backward-compatible aggregate for agent wrapper defaults. New code can import
# ./prompt.nix or ./mcp.nix directly when it needs only one side.
{
  lib,
  ix,
  repoPackages ? { },
  # Rule names dropped from the baked house prompt; forwarded to ./prompt.nix.
  promptOmitRules ? [ ],
}:
let
  prompt = import ./prompt.nix {
    inherit lib;
    omitRules = promptOmitRules;
  };
  mcp = import ./mcp.nix { inherit lib ix repoPackages; };
in
prompt // mcp
