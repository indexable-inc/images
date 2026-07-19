# Backward-compatible aggregate for agent wrapper defaults. New code can import
# ./prompt or ./mcp.nix directly when it needs only one side.
{
  lib,
  ix,
  repoPackages ? {},
  # Rule names dropped from the baked house prompt; forwarded to ./prompt.
  promptOmitRules ? [],
  # Topic names dropped the same way (see ./prompt's `omitTopics`).
  promptOmitTopics ? [],
}: let
  prompt = import ./prompt {
    inherit lib;
    omitRules = promptOmitRules;
    omitTopics = promptOmitTopics;
  };
  mcp = import ./mcp.nix {inherit lib ix repoPackages;};
in
  prompt // mcp
