# Starship prompt settings, pure nix. home-manager renders this attrset to
# ~/.config/starship.toml (store-backed) and nushell loads that file via its
# vendor autoload. Nix rather than a committed TOML so the prompt can bake
# eval-time facts, like the flake pin ages below.
{
  # index pin repo: the submodule checkout under the private config. Its
  # path: input carries no lastModified (git+file fetch canonicalizes
  # mtimes, index#3733), so the segment asks git for the commit epoch at
  # prompt time; it also tracks bumps without a switch.
  indexPinRepo,
  # ix pin: a git flake input, so its commit epoch is real at eval time and
  # baked at switch time.
  ixPinTimestamp,
}: let
  # Ages of the index/ix flake pins shown in the prompt. Each pin's epoch
  # reaches sh as a shell expression and sh computes the age at prompt time,
  # so the segment shows live staleness, not a string frozen at the switch.
  pinStyles = {
    index = {
      icon = "󰏗";
      style = "italic dimmed #8aadf4";
    };
    ix = {
      icon = "󰒓";
      style = "italic dimmed #a6da95";
    };
  };
  # POSIX-sh helper: seconds since a baked epoch rendered as 3d/5h/12m.
  ageFn = ''age() { d=$(( $(date +%s) - $1 )); if [ "$d" -ge 86400 ]; then echo "$((d / 86400))d"; elif [ "$d" -ge 3600 ]; then echo "$((d / 3600))h"; else echo "$((d / 60))m"; fi; }'';
  # epochSh: shell expression producing the pin's commit epoch; a failing
  # substitution (host without the checkout) short-circuits the echo so the
  # segment renders empty instead of a bogus age.
  pinModule = name: epochSh: let
    pin = pinStyles.${name};
  in {
    command = "${ageFn}; epoch=${epochSh} && echo \"${pin.icon} $(age $epoch)\"";
    shell = [
      "sh"
      "-c"
    ];
    use_stdin = false;
    when = true;
    ignore_timeout = true;
    format = "[$output]($style) ";
    inherit (pin) style;
    description = "Age of the ${name} flake pin baked into this config";
  };
in {
  # `$all` covers every module not named here, so the leading modules are
  # spelled out to keep `${custom.vcs}` where `git_branch` used to render:
  # right after the directory, ahead of the language and tooling segments.
  format = "\${custom.submodule_chain}$username$hostname$localip$directory\${custom.vcs}$all$line_break$character";
  aws = {
    disabled = true;
    symbol = "  ";
  };
  buf = {
    disabled = true;
    symbol = " ";
  };
  bun = {
    disabled = true;
    symbol = " ";
  };
  c = {
    symbol = " ";
  };
  cpp = {
    symbol = " ";
  };
  cmake = {
    symbol = " ";
  };
  conda = {
    symbol = " ";
  };
  crystal = {
    symbol = " ";
  };
  dart = {
    symbol = " ";
  };
  deno = {
    symbol = " ";
  };
  # directory: candidate tuning kept from the TOML era:
  #   read_only = " 󰌾"
  #   Always show <org>/<repo>/<subpath> for ~/Projects/<org>/<repo>/...:
  #   substitute the Projects prefix away, then keep enough segments that the
  #   org stays visible inside deep subdirs.
  #   truncation_length = 8; truncate_to_repo = false; truncation_symbol = ""
  directory = {
    substitutions = {
      "~/Projects/" = "";
    };
  };
  docker_context = {
    symbol = " ";
  };
  elixir = {
    symbol = " ";
  };
  elm = {
    symbol = " ";
  };
  fennel = {
    symbol = " ";
  };
  fossil_branch = {
    symbol = " ";
  };
  gcloud = {
    disabled = true;
    symbol = " ";
  };
  # Replaced by `custom.vcs` (the vcs-prompt binary), which renders jj state
  # in a jj workspace and git state everywhere else. Neither module can be
  # disabled per directory, and in a colocated jj repo both report the
  # exported git view: a detached HEAD at whatever `jj git export` last wrote.
  git_branch = {
    disabled = true;
  };
  git_status = {
    disabled = true;
  };
  git_commit = {
    disabled = true;
    format = "[( $tag )]($style)";
    only_detached = false;
    tag_disabled = false;
    tag_symbol = "";
  };
  golang = {
    symbol = " ";
  };
  guix_shell = {
    symbol = " ";
  };
  haskell = {
    symbol = " ";
  };
  haxe = {
    symbol = " ";
  };
  hg_branch = {
    symbol = " ";
  };
  hostname = {
    ssh_symbol = " ";
  };
  java = {
    symbol = " ";
  };
  julia = {
    symbol = " ";
  };
  kotlin = {
    symbol = " ";
  };
  lua = {
    symbol = " ";
  };
  memory_usage = {
    disabled = false;
    format = "via $symbol[$ram( | $swap)]($style) ";
    style = "bold dimmed green";
    symbol = "󰍛 ";
    # Show when memory usage is above 95% (default is 75%)
    threshold = 95;
  };
  meson = {
    symbol = "󰔷 ";
  };
  nim = {
    symbol = "󰆥 ";
  };
  nix_shell = {
    disabled = true;
    symbol = "  ";
    format = "[$symbol]($style)";
  };
  nodejs = {
    disabled = true;
    symbol = " ";
  };
  ocaml = {
    symbol = " ";
  };
  os = {
    symbols = {
      AlmaLinux = " ";
      Alpaquita = " ";
      Alpine = " ";
      Amazon = " ";
      Android = " ";
      Arch = " ";
      Artix = " ";
      CachyOS = " ";
      CentOS = " ";
      Debian = " ";
      DragonFly = " ";
      Emscripten = " ";
      EndeavourOS = " ";
      Fedora = " ";
      FreeBSD = " ";
      Garuda = "󰛓 ";
      Gentoo = " ";
      HardenedBSD = "󰞌 ";
      Illumos = "󰈸 ";
      Kali = " ";
      Linux = " ";
      Mabox = " ";
      Macos = " ";
      Manjaro = " ";
      Mariner = " ";
      MidnightBSD = " ";
      Mint = " ";
      NetBSD = " ";
      NixOS = " ";
      Nobara = " ";
      OpenBSD = "󰈺 ";
      OracleLinux = "󰌷 ";
      Pop = " ";
      Raspbian = " ";
      RedHatEnterprise = " ";
      Redhat = " ";
      Redox = "󰀘 ";
      RockyLinux = " ";
      SUSE = " ";
      Solus = "󰠳 ";
      Ubuntu = " ";
      Unknown = " ";
      Void = " ";
      Windows = "󰍲 ";
      openSUSE = " ";
    };
  };
  package = {
    disabled = true;
    symbol = "󰏗 ";
  };
  perl = {
    symbol = " ";
  };
  php = {
    symbol = " ";
  };
  pijul_channel = {
    symbol = " ";
  };
  pixi = {
    symbol = "󰏗 ";
  };
  python = {
    disabled = true;
    symbol = " ";
  };
  rlang = {
    symbol = "󰟔 ";
  };
  ruby = {
    symbol = " ";
  };
  rust = {
    disabled = true;
    symbol = "󱘗 ";
  };
  scala = {
    symbol = " ";
  };
  swift = {
    disabled = true;
    symbol = " ";
  };
  zig = {
    symbol = " ";
  };
  typst = {
    disabled = true;
  };
  direnv = {
    disabled = true;
    format = "[$symbol$loaded]($style) ";
    symbol = "";
    style = "bold green";
    allowed_msg = "";
    not_allowed_msg = "!";
    denied_msg = "✗";
    loaded_msg = "󰒓";
    unloaded_msg = "";
  };
  gradle = {
    symbol = " ";
  };
  status = {
    symbol = " ";
  };
  shell = {
    disabled = false;
    fish_indicator = "";
    nu_indicator = "";
    format = "[$indicator]($style)";
  };
  custom = {
    claude = {
      command = "echo 🤖";
      description = "Shows when in a Claude Code session";
      format = "[$output]($style) ";
      style = "bold blue";
      when = "nu -n -c 'if (\"CLAUDE_CODE\" in $env) or (\"CLAUDE_SESSION\" in $env) { exit 0 } else { exit 1 }'";
    };
    # The VCS segment, in place of the disabled `git_branch` and `git_status`:
    # `on 󱗆 lsurukvy ix-patched+2 *` in a jj workspace (working-copy change id,
    # nearest bookmark and the distance to it, then conflict / divergent /
    # non-empty flags), `on  main !3?1⇡2` in a git repo, with the symbols
    # those modules were configured with. The binary colors its own output
    # rather than taking a `style`, since one segment carries several colors.
    # `git_state` stays enabled and still reports an in-progress rebase.
    vcs = {
      command = "vcs-prompt";
      when = "vcs-prompt detect";
      shell = [
        "sh"
        "-c"
      ];
      use_stdin = false;
      # Wait the commands out rather than let starship drop the segment at its
      # 500ms mark: warm they cost ~20ms (`jj log`) and ~50ms (`git status`
      # here, three levels of submodules), but a loaded box blows past 500ms
      # and a branch that vanishes exactly when a build is running is worse
      # than a prompt that arrives late. Waiting is safe because neither
      # command takes a repo lock: `--ignore-working-copy` skips jj's
      # working-copy snapshot, `--no-optional-locks` skips git's index
      # refresh.
      ignore_timeout = true;
      format = "$output ";
      description = "jj working-copy state in a jj workspace, git branch and status elsewhere";
    };

    # Human-readable time since the latest commit (e.g. "2 hours ago"), shown
    # to the left of newer custom modules. Runs under sh because when starship
    # spawns `nu -c`, external commands in the pipeline don't inherit the
    # working directory starship sets; sh passes it to all children.
    # %cr is git's committer-date relative format; exits non-zero with no
    # commits.
    git_commit_age = {
      command = "git log -1 --pretty=format:%cr";
      shell = [
        "sh"
        "-c"
      ];
      use_stdin = false;
      require_repo = true;
      when = true;
      ignore_timeout = true;
      format = "[$output]($style) ";
      style = "italic dimmed";
      description = "Relative time since the latest git commit";
    };

    # Breadcrumb of the git superproject chain, shown only when the cwd is
    # inside a submodule. This config nests three repos (config > ix > index,
    # nix#147 / ix#8119), and truncate_to_repo collapses the prompt to just the
    # innermost repo name, hiding where you actually are. The module walks
    # `--show-superproject-working-tree` outward and joins the ancestor repo
    # basenames with a dimmed chevron, so `index/packages` reads as
    # `nix › ix › index/packages`. Ancestors only; the innermost repo is the
    # directory module that follows. Gating is free: require_repo skips
    # non-repo dirs natively, and the conditional format group drops the
    # module when the walk prints nothing (empty output alone does not hide a
    # custom module; it would leave a stray chevron). Runs under sh for the
    # same cwd-inheritance reason as git_commit_age above.
    submodule_chain = {
      command = ''sup=$(git rev-parse --show-superproject-working-tree 2>/dev/null); names=""; while [ -n "$sup" ]; do b=$(basename "$sup"); if [ -z "$names" ]; then names="$b"; else names="$b › $names"; fi; sup=$(git -C "$sup" rev-parse --show-superproject-working-tree 2>/dev/null); done; printf "%s" "$names"'';
      shell = [
        "sh"
        "-c"
      ];
      use_stdin = false;
      require_repo = true;
      when = true;
      ignore_timeout = true;
      format = "([$output ›]($style) )";
      style = "dimmed";
      description = "Breadcrumb of git superproject repos when inside a submodule";
    };

    # Retired custom modules, kept from the TOML era for reference:
    #
    # prs: open PRs by me in this repo, via the Nushell cache. Custom modules
    # run under the default Starship command shell; keep prompt modules
    # local-only, Nushell background jobs refresh any external state.
    #   command = "nu -n -c 'use .../nushell/functions/github.nu *; github-pr-prompt'"
    #
    # todos: count of TODOs in the codebase. nushell doesn't work here because
    # when starship spawns `nu -c`, the working directory isn't properly
    # inherited by external commands (like rg) in the pipeline; sh -c works
    # because sh passes the working directory to all child processes.
    #   command = "rg -ic todo -g '!vendored/**' . | awk -F: '{s+=$NF} END {if(s>0) print s}'"
    #
    # ix: command = "ix-starship" (branch and sync status: ↻ syncing, ⏸ stopped, ✗ crashed)
    flake_pin_index = pinModule "index" "$(git -C '${indexPinRepo}' log -1 --format=%ct 2>/dev/null)";
    flake_pin_ix = pinModule "ix" (toString ixPinTimestamp);
  };
}
