# Git, ported from dots/git in the source repo. The forgejo credential
# helpers and the theme/delta include files are secret- and theme-machinery
# and are intentionally not ported. The generalizable halves come from
# upstream home-manager modules instead of hand-written config sections:
# programs.diff-so-fancy owns the pager/diffFilter wiring, and
# programs.gh.gitCredentialHelper (see ./home.nix) owns the github
# credential helper. What stays here is hari's: identity, ssh signing, the
# color taste, and his global ignore list.
_: {
  programs.diff-so-fancy = {
    enable = true;
    # Configures diff-so-fancy as git's diff/log/show pager (the default
    # pagerOpts match the source config's `less --tabs=4 -RFX`) and as the
    # interactive.diffFilter.
    enableGitIntegration = true;
    settings = {
      markEmptyLines = true;
      stripLeadingSymbols = true;
      useUnicodeRuler = true;
    };
  };

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
      };

      feature.manyFiles = true;

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

      push.autoSetupRemote = true;

      user = {
        name = "Harivansh Rathi";
        email = "rathiharivansh@gmail.com";
      };
    };
  };
}
