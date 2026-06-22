{ index }:

index.lib.mkFleet {
  defaults = [ { ix.image.tag = "synced-github-auth"; } ];

  # One GitHub token, declared once for the whole fleet. `ix.secrets`
  # normalizes this into a runtime path every node sees at the same place
  # (`/run/secrets/github/token`). Only that path enters the Nix store; the
  # token bytes are written at runtime by the ix secrets manager
  # (https://github.com/indexable-inc/index/issues/66), still in progress.
  #
  # Point `key` at wherever the token lives in the provider. A fine-grained
  # GitHub PAT or a GitHub App installation token scoped to the repos the
  # agents touch is the right credential here, not a personal classic PAT.
  secrets = {
    provider = {
      type = "vaultwarden";
      client = "rbw";
      server = "https://vaultwarden.internal.example";
      mountRoot = "/run/secrets";
      folder = "production";
    };
    "github/token".key = "github/agent-token";
  };

  # Three interchangeable agent VMs. None of them runs `gh auth login`; they
  # all consume the same declared token, so adding a fourth replica costs
  # nothing.
  nodes.agent = {
    replicas = 3;
    modules = [ ./agent.nix ];
  };
}
