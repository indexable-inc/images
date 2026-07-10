# Git, ported from dots/git in the source repo. The forgejo credential
# helpers and the theme/delta include files are secret- and theme-machinery
# and are intentionally not ported; the github credential helper comes from
# programs.gh.gitCredentialHelper (see ./home.nix) instead of a hand-written
# `!gh auth git-credential` section.
{
  lib,
  pkgs,
  ...
}: {
  programs.git = {
    enable = true;

    lfs.enable = true;

    signing = {
      key = "~/.ssh/id_ed25519.pub";
      format = "ssh";
      signByDefault = true;
    };

    # Written to ~/.config/git/ignore, which the source config pointed
    # core.excludesFile at.
    ignores = [
      "*.swp"
      "*.swo"
      "*~"
      ".DS_Store"
      "Thumbs.db"
      ".env"
      ".env.local"
      ".env.*.local"
      ".vscode/"
      ".idea/"
      # Track committed .claude content (agents, hooks, settings.json) across
      # all repos; only the per-machine local override stays out of git.
      ".claude/settings.local.json"
      "CLAUDE.md"
      "node_modules/"
      "__pycache__/"
      "*.pyc"
      "venv/"
      ".venv/"
      "build/"
      "dist/"
      "out/"
      "target/"
      "result"
      "result-*"
      ".direnv/"
    ];

    settings = {
      advice.detachedHead = false;

      core = {
        editor = "nvim";
        fsmonitor = true;
        pager = "${lib.getExe pkgs.diff-so-fancy} | less --tabs=4 -RFX";
      };

      feature.manyFiles = true;

      interactive.diffFilter = "${lib.getExe pkgs.diff-so-fancy} --patch";

      color = {
        ui = true;

        diff = {
          meta = 11;
          frag = "magenta bold";
          func = "146 bold";
          commit = "yellow bold";
          old = "red bold";
          new = "green bold";
          whitespace = "red reverse";
        };

        diff-highlight = {
          oldNormal = "red bold";
          oldHighlight = "red bold 52";
          newNormal = "green bold";
          newHighlight = "green bold 22";
        };
      };

      diff-so-fancy = {
        markEmptyLines = true;
        stripLeadingSymbols = true;
        useUnicodeRuler = true;
      };

      push.autoSetupRemote = true;

      user = {
        name = "Harivansh Rathi";
        email = "rathiharivansh@gmail.com";
      };
    };
  };
}
