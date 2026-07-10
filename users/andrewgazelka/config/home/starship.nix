# Starship prompt settings, pure nix. home-manager renders this attrset to
# ~/.config/starship.toml (store-backed) and nushell loads that file via its
# vendor autoload. Nix rather than a committed TOML so the prompt can bake
# eval-time facts, like the flake pin ages below.
{pinTimestamps}: let
  # Ages of the index/ix flake pins shown in the prompt. Each pin's commit
  # epoch is baked at switch time and sh computes the age at prompt time, so
  # the segment shows live staleness, not a string frozen at the last switch.
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
  pinModule = name: timestamp: let
    pin = pinStyles.${name};
  in {
    command = "${ageFn}; echo \"${pin.icon} $(age ${toString timestamp})\"";
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
  format = "$all$line_break$character";
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
  git_branch = {
    symbol = " ";
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
  git_status = {
    format = "([$all_status$ahead_behind]($style) )";
    stashed = "";
    ahead = "⇡\${count}";
    behind = "⇣\${count}";
    diverged = "⇕⇡\${ahead_count}⇣\${behind_count}";
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
    # jj: command = "jj-starship"; detect_folders = [".jj"]
    # ix: command = "ix-starship" (branch and sync status: ↻ syncing, ⏸ stopped, ✗ crashed)
    flake_pin_index = pinModule "index" pinTimestamps.index;
    flake_pin_ix = pinModule "ix" pinTimestamps.ix;
  };
}
