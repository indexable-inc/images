# Index-free interactive development baseline. Keep this profile independent
# of inputs.index so the builder VM can build the index toolchain itself.
{
  agentLua,
  configRoot,
}: {
  config,
  lib,
  pkgs,
  ...
}: let
  nvimLua = pkgs.runCommand "andrewgazelka-nvim-lua" {__structuredAttrs = true;} ''
    cp -R ${configRoot + "/nvim/lua"} "$out"
    chmod -R u+w "$out"
    mkdir -p "$out/agent"
    install -Dm644 ${agentLua} "$out/agent/shared.lua"
  '';
in {
  xdg.configFile."nvim/lua".source = nvimLua;

  home.file = {
    ".config/nushell" = lib.mkIf pkgs.stdenv.isLinux {
      source = configRoot + "/nushell";
    };
    ".config/nushell-hm-session-vars.nu".text =
      lib.concatStringsSep "\n" (
        lib.mapAttrsToList (
          name: value: "$env.${name} = ${builtins.toJSON (toString value)}"
        )
        config.home.sessionVariables
      )
      + "\n";
  };

  programs = {
    bash.enable = true;
    neovim = {
      enable = true;
      defaultEditor = true;
      viAlias = true;
      vimAlias = true;
      withRuby = false;
      withPython3 = false;
      initLua = builtins.readFile (configRoot + "/nvim/init.lua");
    };
    zoxide.enable = true;
    direnv = {
      enable = true;
      nix-direnv.enable = true;
    };
    fzf = {
      enable = true;
      historyWidget.command = "";
    };
    starship.enable = true;
    atuin.enable = true;
  };
}
