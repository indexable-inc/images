{
  lib,
  tmux,
  symlinkJoin,
  makeWrapper,
  writeText,
}: let
  directive = lib.concatStringsSep " ";
  tmuxConfig = writeText "index-tmux.conf" (
    lib.concatMapStringsSep "\n" directive [
      ["set" "-g" "default-terminal" ''"tmux-256color"'']
      ["set" "-as" "terminal-features" ''",*:RGB"'']
      ["set" "-as" "terminal-features" ''",*:usstyle"'']
      ["set-environment" "-g" "CLAUDE_CODE_TMUX_TRUECOLOR" "1"]
      ["set" "-sg" "escape-time" "10"]
      ["set" "-g" "focus-events" "on"]
      ["set" "-g" "repeat-time" "600"]
      ["set" "-g" "mouse" "on"]
      ["set" "-g" "history-limit" "50000"]
      ["set" "-g" "set-clipboard" "on"]
      ["set" "-g" "base-index" "1"]
      ["setw" "-g" "pane-base-index" "1"]
      ["set" "-g" "renumber-windows" "on"]
      ["setw" "-g" "mode-keys" "vi"]
      ["set" "-g" "allow-passthrough" "on"]
      ["source-file" "-q" "~/.config/tmux/tmux.conf"]
      ["source-file" "-q" "~/.tmux.conf"]
    ]
    + "\n"
  );
in
  # tmux with modern defaults baked in (truecolor, undercurl, mouse, vi copy mode,
  # sane history/escape-time). `-f` points at our config, which sources the user's
  # own ~/.config/tmux/tmux.conf last so personal settings still win. symlinkJoin
  # (not a bare wrapper) folds tmux's man pages into the single `out` so they
  # survive the wrap.
  symlinkJoin {
    name = "tmux-${tmux.version}";
    # Include tmux.man explicitly: symlinkJoin only merges each input's default
    # output, so without this the man pages (tmux's separate `man` output) are lost.
    paths = [
      tmux
      tmux.man
    ];
    nativeBuildInputs = [makeWrapper];
    postBuild = ''
      # shell
      wrapProgram $out/bin/tmux --add-flags "-f ${tmuxConfig}"
    '';
    meta =
      tmux.meta
      // {
        description = "${tmux.meta.description}, with modern truecolor defaults baked in";
        mainProgram = "tmux";
        # This derivation has only `out` (man pages folded in above). Base tmux's
        # meta lists outputsToInstall = [ "out" "man" ]; keeping `man` makes buildenv
        # (e.g. home.packages) try to read a nonexistent `man` output and fail.
        outputsToInstall = ["out"];
      };
  }
