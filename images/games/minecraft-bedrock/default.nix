# Minecraft Bedrock server image.
{ config, ... }:
{
  ix.image = {
    name = "minecraft-bedrock";
    # Tag tracks the pinned Bedrock package version, so bumping the server
    # version in modules/services/minecraft-bedrock moves the image tag.
    tag = config.services.minecraft-bedrock.package.version;
  };

  services.minecraft-bedrock = {
    enable = true;
    settings = {
      server-name = "ix-powered Bedrock";
      max-players = 20;
    };
  };
}
