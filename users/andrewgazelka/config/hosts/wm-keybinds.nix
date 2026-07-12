# Single source of truth for the portable window-manager keymap. AeroSpace
# (macOS / hydra) is the live consumer. The `swayBindings` output is retained
# for a future Linux GUI guest (the current hosts/vm guest is headless), so both
# would tile with the same muscle memory. Only the WM-intrinsic, portable actions
# live here (focus, move, workspaces); platform-only bits stay in each consumer —
# macOS extras (on-window-detected, resize, join-with) in profiles/workstation.nix.
#
# Per-platform primary modifier (chosen for idiomatic, conflict-free keys): the
# `primary` token renders to Option/Alt on macOS and Super ($mod = Mod4) on sway.
# Same letters and numbers on both; only the held modifier differs.
{lib}: let
  dirs = [
    {
      key = "h";
      word = "left";
    }
    {
      key = "j";
      word = "down";
    }
    {
      key = "k";
      word = "up";
    }
    {
      key = "l";
      word = "right";
    }
  ];
  wsNums = lib.range 1 9;

  # Each binding: mods (subset of [ "ctrl" "primary" "shift" ] in that canonical
  # order, matching AeroSpace's "ctrl-alt-shift-h" spelling) + key + the command
  # in each WM. Derived from the existing AeroSpace binds so macOS is unchanged.
  bindings =
    (map (d: {
        mods = ["primary"];
        inherit (d) key;
        aerospace = "focus --boundaries-action stop --ignore-floating ${d.word}";
        sway = "focus ${d.word}";
      })
      dirs)
    ++ (map (d: {
        mods = [
          "ctrl"
          "primary"
          "shift"
        ];
        inherit (d) key;
        aerospace = "move ${d.word}";
        sway = "move ${d.word}";
      })
      dirs)
    ++ (map (n: {
        mods = ["primary"];
        key = toString n;
        aerospace = "workspace ${toString n}";
        sway = "workspace number ${toString n}";
      })
      wsNums)
    ++ (map (n: {
        mods = [
          "primary"
          "shift"
        ];
        key = toString n;
        aerospace = "move-node-to-workspace ${toString n}";
        sway = "move container to workspace number ${toString n}";
      })
      wsNums);

  # primary token → per-platform modifier spelling.
  aeroMod = m:
    {
      primary = "alt";
      shift = "shift";
      ctrl = "ctrl";
    }.${
      m
    };
  swayMod = m:
    {
      primary = "$mod";
      shift = "Shift";
      ctrl = "Ctrl";
    }.${
      m
    };

  aeroChord = b: lib.concatStringsSep "-" (map aeroMod b.mods ++ [b.key]);
  swayChord = b: lib.concatStringsSep "+" (map swayMod b.mods ++ [b.key]);
in {
  # For programs.aerospace.settings.mode.main.binding (an attrset of chord → cmd).
  aerospaceBindings = lib.genAttrs' bindings (b: lib.nameValuePair (aeroChord b) b.aerospace);

  # For the sway config: a block of indented `bindsym <chord> <cmd>` lines.
  swayBindings = lib.concatMapStringsSep "\n" (b: "    bindsym ${swayChord b} ${b.sway}") bindings;
}
