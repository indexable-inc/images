_:

{
  # Shared Hermes agent profile consumed by examples and downstream fleets.
  profile = ./profile.nix;

  documents = {
    operator = {
      "USER.md" = ./documents/operator/USER.md;
      "SOUL.md" = ./documents/operator/SOUL.md;
    };
    telegram = {
      "SOUL.md" = ./documents/telegram/SOUL.md;
    };
    minecraftOperator = {
      "SOUL.md" = ./documents/minecraft-operator/SOUL.md;
    };
  };
}
