{ index }:

index.lib.mkFleet {

  # One GitHub token, declared once for the whole fleet. The ix account secret
  # store owns the lower snake_case key; this fleet declares how VMs receive it
  # at runtime as `/run/secrets/github/token`.
  #
  # Store it first with `ix secret set github_token`. A fine-grained GitHub PAT
  # or GitHub App installation token scoped to the repos the agents touch is
  # the right credential here, not a personal classic PAT.
  deployment.secrets.github_token = {
    file = "github/token";
    owner = "root";
    mode = "0400";
  };

  # Three interchangeable agent VMs. None of them runs `gh auth login`; they
  # all consume the same declared token, so adding a fourth replica costs
  # nothing.
  nodes.agent = {
    replicas = 3;
    modules = [ ./agent.nix ];
  };
}
