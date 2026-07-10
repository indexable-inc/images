# Neovim, ported from dots/nvim in the source repo. The lua tree deploys
# verbatim under ./config/nvim (plugins install at runtime via vim.pack +
# lz.n, so no nix plugin wiring is needed); this module supplies init.lua and
# the language servers / tools the config expects on PATH. In the source repo
# the same list lived in modules/users/user-config/packages.nix.
#
# Closed over `ix` for the shared language-toolchain handles: the pinned Go
# and Elixir selections are repo-wide policy (lib/languages/), not per-user
# package spelling.
{ix}: {pkgs, ...}: let
  jsonFormat = pkgs.formats.json {};
in {
  programs.neovim = {
    enable = true;
    defaultEditor = true;
    viAlias = true;
    vimAlias = true;
    vimdiffAlias = true;
    withRuby = false;
    withPython3 = false;
    withNodeJs = false;

    initLua = builtins.readFile ./config/nvim/init.lua;

    extraPackages = [
      pkgs.bat
      pkgs.clang
      pkgs.clang-tools
      pkgs.fd
      pkgs.fzf
      pkgs.gh
      pkgs.git
      pkgs.gopls
      pkgs.lua-language-server
      pkgs.pyright
      pkgs.python3
      pkgs.ripgrep
      pkgs.stylua
      pkgs.tree-sitter
      pkgs.vscode-langservers-extracted
      pkgs.bash-language-server
      pkgs.typescript
      pkgs.typescript-language-server
      (ix.languages.go.toolchain pkgs {version = "1.26";})
      (ix.languages.elixir.toolchain pkgs {version = "1.19";})
      (ix.languages.elixir.languageServer pkgs {})
    ];
  };

  xdg.configFile = {
    "nvim/lua".source = ./config/nvim/lua;
    "nvim/plugin".source = ./config/nvim/plugin;
    "nvim/after".source = ./config/nvim/after;
    "nvim/doc".source = ./config/nvim/doc;

    # dots/nvim/.neoconf.json in the source repo, as nix.
    "nvim/.neoconf.json".source = jsonFormat.generate "harivansh-afk-neoconf.json" {
      neodev.library = {
        enabled = true;
        plugins = true;
      };
      neoconf.plugins.lua_ls.enabled = true;
      lspconfig.lua_ls."Lua.format.enable" = false;
    };
  };

  # `view` opens read-only, matching the nvim command-alias package the
  # source repo installed alongside vi/vim/vimdiff.
  home.shellAliases.view = "nvim -R";
}
