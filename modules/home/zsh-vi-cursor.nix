# Cursor-shape feedback for zsh vi mode, hoisted out of users/harivansh-afk
# (ported there from github:harivansh-afk/nix) because it carries no user
# taste: a beam cursor in insert mode, a block cursor in command mode, and a
# reset to beam around every prompt and command so a full-screen program
# that changes the cursor never leaks its shape back into the shell.
# Option-gated like the other shared home modules: import it and set
# `zshViCursor.enable = true` alongside `programs.zsh.defaultKeymap =
# "viins"` (or any vi keymap).
{
  config,
  lib,
  ...
}: let
  cfg = config.zshViCursor;
in {
  options.zshViCursor = {
    enable = lib.mkEnableOption "cursor-shape feedback for zsh vi mode (beam insert, block command)";
  };

  config = lib.mkIf cfg.enable {
    programs.zsh.initContent = ''
      # --- vi-mode cursor shape: beam for insert, block for command ---
      autoload -Uz add-zle-hook-widget
      _vi_cursor() { printf '\e[%s q' "''${1:-6}"; }
      _vi_cursor_select() { [[ "$KEYMAP" == vicmd ]] && _vi_cursor 2 || _vi_cursor 6; }
      _vi_cursor_beam() { _vi_cursor 6; }
      add-zle-hook-widget zle-keymap-select _vi_cursor_select
      add-zle-hook-widget zle-line-init _vi_cursor_beam
      add-zle-hook-widget zle-line-finish _vi_cursor_beam
      # Composable hooks (not bare precmd()/preexec() definitions) so this
      # shared module never clobbers a consumer's own hook functions.
      autoload -Uz add-zsh-hook
      add-zsh-hook precmd _vi_cursor_beam
      add-zsh-hook preexec _vi_cursor_beam
    '';
  };
}
