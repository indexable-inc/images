# Public facade for the shared MCP registry and renderers.
{lib}: let
  servers = import ./mcp/servers.nix {inherit lib;};
  renderers = import ./mcp/renderers.nix {
    inherit lib;
    toml = import ./toml.nix {inherit lib;};
  };
in
  servers // renderers
