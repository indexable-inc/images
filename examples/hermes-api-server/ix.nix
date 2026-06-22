{ index }:

# The Hermes operator VM exposing the OpenAI-compatible `hermes
# api-server` to sibling VMs. Chat frontends (LobeChat, Open WebUI,
# LibreChat) running in the same east-west group point their OpenAI
# base URL at this node and get the full agent (tools, memory, persona)
# behind a plain chat-completions endpoint. See README.md.
let
  # Any VM that should be able to call the API joins this group; a node
  # outside it has no east-west route or DNS name to the listener.
  eastWestGroup = "hermes-api";
in
index.lib.mkFleet {
  defaults = [ { ix.image.tag = "hermes-api-server"; } ];

  nodes.hermes = {
    groups = [ eastWestGroup ];
    modules = [
      index.lib.hermesAgent.nixosModules.default
      (index.lib.paths.examples + "/hermes-agent/hermes.nix")
      ./api-server.nix
    ];
  };
}
