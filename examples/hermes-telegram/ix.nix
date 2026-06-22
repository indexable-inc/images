{ index }:

# The Hermes operator VM, tuned as a Telegram chat companion. Same node
# shape as examples/hermes-agent (one outbound-only VM, secrets dropped
# at /run/secrets/hermes.env), plus the long-poll Telegram platform and
# a chat-tuned persona. See README.md for the BotFather walkthrough.
index.lib.mkFleet {
  defaults = [ { ix.image.tag = "hermes-telegram"; } ];

  nodes.hermes = {
    modules = [
      index.lib.hermesAgent.nixosModules.default
      (index.lib.paths.examples + "/hermes-agent/hermes.nix")
      ./telegram.nix
    ];
  };
}
