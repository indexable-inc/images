{
  config,
  lib,
  pkgs,
  ...
}: let
  hosts = config.users.andrewgazelka.infra.hosts;
  hostRows = lib.mapAttrsToList (name: host: host // {inherit name;}) hosts;
  json = pkgs.formats.json {};
in {
  home.file.".config/infra/hosts.json".source = json.generate "infra-hosts.json" hostRows;
  home.packages = [pkgs.coreutils];
}
