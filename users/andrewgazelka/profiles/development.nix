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
    install -Dm644 ${agentLua} "$out/agent/init.lua"
  '';
in {
  xdg.configFile."nvim/lua".source = nvimLua;

  # theme.nu in the shared nushell config calls vivid at every login; ship
  # it with the config so index-free hosts (builder VM) get it too (#3165).
  home.packages = [pkgs.vivid];

  home.file = {
    ".config/nushell-hm-session-vars.nu".text =
      lib.concatStringsSep "\n" (
        lib.mapAttrsToList (
          name: value: "$env.${name} = ${builtins.toJSON (toString value)}"
        )
        config.home.sessionVariables
      )
      + "\n";
  }
  # Link nushell config children individually rather than the whole dir:
  # nushell creates history.sqlite3 inside its config dir on Linux, so a
  # single read-only store symlink breaks interactive login (#3165).
  // lib.optionalAttrs pkgs.stdenv.isLinux (
    lib.mapAttrs' (
      name: _:
        lib.nameValuePair ".config/nushell/${name}" {
          source = configRoot + "/nushell/${name}";
        }
    ) (builtins.readDir (configRoot + "/nushell"))
  );

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
