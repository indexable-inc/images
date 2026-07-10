# Full personal workstation profile. Index-owned dependencies are closed over
# by the flake export; host-owned values arrive through typed options.nix.
{
  configRoot,
  indexPackages,
  ix,
  mutableJsonModule,
  optionsModule,
  personalServicesModule,
  provenanceModule,
  indexSkillsSrc,
  tmuxModule,
}: {
  config,
  pkgs,
  lib,
  ...
}: let
  cfg = config.users.andrewgazelka;
  indexPkgs = indexPackages pkgs.stdenv.hostPlatform.system;
  configDir = cfg.paths.privateConfigDirectory;
  inherit (pkgs.python313Packages) fonttools genson shodan termgraph;

  # Tracked configuration is generation-owned on every host. Writable runtime
  # state and secrets are explicit exceptions below, never a platform-wide
  # escape hatch that silently changes Home Manager's ownership model.
  repoRoot = configRoot;
  repoFile = rel: repoRoot + "/${rel}";
  structured = import (configRoot + "/settings/structured.nix");
  yamlStructured = import (configRoot + "/settings/yaml.nix");
  jsonFormat = pkgs.formats.json {};
  tomlFormat = pkgs.formats.toml {};
  iniFormat = pkgs.formats.ini {};
  yamlFormat = pkgs.formats.yaml {};
  renderStructured = name: let
    entry = structured.${name};
    format =
      if entry.format == "json"
      then jsonFormat
      else if entry.format == "toml"
      then tomlFormat
      else throw "unsupported structured config format: ${entry.format}";
  in
    format.generate "andrewgazelka-${name}.${entry.format}" entry.value;
  renderLines = name: lines:
    pkgs.writeText name (lib.concatLines lines);
  renderBtop = name: options:
    pkgs.writeText name (lib.concatLines (lib.mapAttrsToList (
        key: value: let
          rendered =
            if builtins.isBool value
            then lib.boolToString value
            else if builtins.isString value
            then ''"${value}"''
            else toString value;
        in "${key} = ${rendered}"
      )
      options));
  renderKitty = name: options:
    renderLines name (lib.mapAttrsToList (key: value: "${key} ${value}") options);

  # Portable WM keymap for AeroSpace (one source of truth, hosts/wm-keybinds.nix).
  wm = import (configRoot + "/hosts/wm-keybinds.nix") {inherit lib;};

  # Enables `clip copy`/`clip paste` without rebuilding nushell; `native-clip`
  # is a runtime experimental option, not a Nix/Cargo feature.
  nushellWithNativeClip = pkgs.symlinkJoin {
    name = "nushell-with-native-clip";
    paths = [pkgs.nushell];
    nativeBuildInputs = [pkgs.makeWrapper];
    postBuild = ''
      # shell
      wrapProgram $out/bin/nu \
        --add-flags --experimental-options \
        --add-flags '[native-clip=true]'
    '';
  };

  # Purges cookies for the firewall blocklist (flake.nix `blockedHosts`) so a
  # DNS-blocked site is also a logged-out one. Live over CDP for a running
  # browser that exposes a debug port (Dia: 9223), direct sqlite for closed
  # ones. Runs on every switch, so blocked == cookie-free for as long as it is
  # blocked. See scripts/clear-blocked-cookies.py.
  clearBlockedCookies = ix.writeBashApplication pkgs {
    name = "clear-blocked-cookies";
    text = ''exec ${pkgs.python3.interpreter} ${configRoot + "/scripts/clear-blocked-cookies.py"} "$@"'';
  };

  # Override fish to skip failing tests
  fishNoTests = pkgs.fish.overrideAttrs (_: {
    doCheck = false;
  });

  # Personal-only Claude config, shipped through the index claude-code
  # wrapper's read-only `--settings` (flagSettings) layer via `extraSettings`
  # below. flagSettings merges per-key ABOVE the user settings.json and is a
  # SEPARATE read-only layer, so nothing here ever has to symlink or copy the
  # writable ~/.claude/settings.json the CLI churns at runtime; that sidesteps
  # Claude Code's settings-symlink perms/perf bugs (anthropics/claude-code#3575,
  # #58443, #55485) with no `install`/copy hack.
  # House posture lives in the index wrapper itself now: attribution, worktree
  # baseRef, effort/fast/theme runtime-toggle defaults, auto-updates channel,
  # the version-aware statusline, the 1M/cron/autocompact clamps (typed
  # `features` argument), and the built-in tool deny rules all come from
  # packages/agent/claude-code in indexable-inc/index (#2449); `extraSettings`
  # merges over those defaults, so anything restated here is an override, and
  # only what is genuinely personal (paths, plugins, marketplaces) belongs
  # here. `model` lives in ~/.claude.json, not here, so the /model picker is
  # unaffected. Lifecycle hooks are index-owned too (packages/agent/hooks.nix):
  # baked into the claude-code package (Claude) and exposed as
  # `codex.passthru.hooksJson` (Codex, delivered to ~/.codex/hooks.json below).

  claudeSettings = {
    autoMemoryDirectory = "~/.config/nix/claude/auto-memory";
    enabledPlugins = {
      "ix-docs@ix" = true;
      "ix@ix" = true;
    };
    extraKnownMarketplaces = {
      Mixedbread-Grep.source = {
        source = "github";
        repo = "mixedbread-ai/mgrep";
      };
      antithesis-skills.source = {
        source = "github";
        repo = "antithesishq/antithesis-skills";
      };
      ix.source = {
        source = "github";
        repo = "indexable-inc/docs";
      };
    };
    # hooks are NOT set here: both agents' hooks are now owned by the index repo
    # (packages/agent/hooks.nix) — Claude's are baked into the claude-code
    # package, Codex's come from `codexBase.passthru.hooksJson` below.
  };

  # claude-code from the index FLAKE PACKAGE SET (packages/claude-code in the
  # indexable-inc/index monorepo), not the overlay's pkgs.claude-code: only the
  # package-set build can reach its `mcp` sibling (`repoPackages`). This config
  # overrides the package default MCP set so Claude and Codex expose the same
  # house-approved servers. A user `--mcp-config` on the CLI or a project
  # `.mcp.json` still merges on top. The wrapper runs the default
  # bypass-permissions posture. extraSettings ships our whole static config
  # through the wrapper's read-only --settings layer (see claudeSettings
  # above). The ix-mcp kernel binds loopback via the device-level IX_MCP_HOST
  # session var (see home.sessionVariables), so Claude and Codex match.
  claudeCode = indexPkgs.claude-code.override {
    extraSettings = claudeSettings;
    # Shared registry rendered to Claude's MCP JSON, filtered below for local
    # tool policy.
    mcpServers = ixMcp.toClaudeJson agentMcpServers;
    # Skills no longer ride a baked `--plugin-dir` plugin: they are delivered
    # bare to ~/.claude/skills by the upstream programs.claude-code module's
    # `skills` option (see programs.claude-code below). Hooks are baked by the
    # package itself; agents/commands ride bare via the module. Nothing else
    # used the plugin, so no `pluginDirs` here anymore.
    # The worktree-guard's protected primary checkouts. The package default is
    # `/home/*/{index,ix}` (Linux fleet); on this Mac the long-lived checkouts
    # live under ~/Projects, so point the guard there.
    primaryCheckouts = [
      "/Users/*/Projects/*/index"
      "/Users/*/Projects/*/ix"
    ];
    # appendSystemPrompt (house rules appended to the stock prompt) comes from
    # the package default. Set `appendSystemPrompt = null;` here to ship the
    # stock prompt alone on this machine.
  };

  # House MCP registry, index/lib/util/mcp.nix, is the SINGLE source both
  # agents render from. Keep the local policy as a filter over the shared
  # registry so transport details cannot drift between Claude and Codex.
  ixMcp = ix.mcp;
  agentMcpServers = lib.removeAttrs (
    ixMcp.defaultServers {
      indexCommand = lib.getExe indexPkgs.mcp;
    }
  ) ["exa"];
  houseHttpServers =
    lib.filterAttrs (
      _: def:
        (
          def.transport or "stdio"
        )
        != "stdio"
    )
    agentMcpServers;

  # Shared skill source: the index repo's SKILL.md bundles (open Agent-Skills
  # standard, `packages/agent/skills`). ONE directory, delivered to BOTH agents
  # bare (no plugin namespace, so `/<skill>` on Claude and `$<skill>` / implicit
  # on Codex): Claude via `programs.claude-code.skills`, Codex via the upstream
  # `programs.codex.skills`. Replaces the old per-agent Claude plugin wrapper.
  # `.outPath` (an unambiguous store-path string), NOT the bare flake-input
  # attrset: both modules branch on `builtins.isAttrs skills` to choose
  # attrset-of-skills vs directory mode, and a flake input is an attrset.
  skillsSrc = indexSkillsSrc;

  # Our own Codex: the index codex wrapper (operational defaults + the stdio
  # `index` MCP server baked in), re-`.override`n to carry THIS machine's
  # declarative config as codex `-c` flags. Those flags are codex's
  # highest-precedence layer (above ~/.codex/config.toml), so they are the
  # nix-managed config layer and ~/.codex/config.toml is left as codex's own
  # mutable runtime file (project trust, desktop settings, notices) rather than
  # a repo symlink it churns into git.
  #   - `settings` (soft): injected only when that exact key is absent from
  #     config.toml, so the user can still change model/effort/etc. in the TUI
  #     and have it persist.
  #   - `forcedSettings`: applied on every run — wrapper invariants and the
  #     safety posture that must not silently drift.
  # Only scalar leaves can be baked (the package renders via toml.scalar, which
  # rejects lists): `notify` (a list) and the `[notice]` migration keys (dots in
  # the key names) stay in codex's mutable config.toml.
  codexBase = indexPkgs.codex;
  codex = codexBase.override {
    # Neutral defs; the package renders them itself (stdio baked, http filtered
    # out and re-added via settings.mcp_servers below). Same set Claude bakes.
    mcpServers = agentMcpServers;
    settings = {
      # index codex package defaults, restated (`.override` replaces the arg).
      features.multi_agent_v2 = {
        enabled = true;
        max_concurrent_threads_per_session = 16;
      };
      agents.max_depth = 3;
      # http MCP servers from the filtered shared registry. stdio `index` is
      # already baked by codexBase.
      mcp_servers = lib.mapAttrs (_: def: {inherit (def) url;}) houseHttpServers;
      # Operational defaults the user may still override live in the TUI.
      model = "gpt-5.5";
      model_reasoning_effort = "low";
      personality = "pragmatic";
      service_tier = "fast";
    };
    forcedSettings = {
      check_for_update_on_startup = false;
      bypass_hook_trust = true;
      sandbox_mode = "danger-full-access";
      default_permissions = ":danger-full-access";
      commit_attribution = "";
      features = {
        steer = true;
        multi_agent = true;
        apps = false;
        plugins = false;
        terminal_resize_reflow = true;
        goals = true;
        # Off: the "external config migration" is what silently injected the
        # dead `ix → 127.0.0.1:55444` MCP server into config.toml. Disabling it
        # stops codex rewriting MCP config behind our back (root-cause fix).
        external_migration = false;
        js_repl = false;
      };
      shell_environment_policy = {
        "inherit" = "all";
        ignore_default_excludes = true;
        set = {
          CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS = "1";
        };
      };
    };
  };
in {
  # Personal-but-shareable workstation module hoisted into the `index` monorepo
  # (users/andrewgazelka): pr-watch, optimize-scan, lifelog, and shared
  # workstation services. Overlay and downtime pieces are disabled in
  # profiles/darwin-home.nix. It pulls in `homeModules.portable-services` transitively, so
  # the generic service layer is still in scope for host-specific agents.
  #
  # The portable and development profiles are the explicit, index-free
  # user baseline shared with the headless VM. Workstation and index tooling
  # stay in this macOS consumer until their dedicated profiles are extracted.
  imports = [
    optionsModule
    personalServicesModule
    # Per-generation provenance manifest: each HM generation carries
    # provenance.json mapping deployed files back to the nix file:line that
    # defined them; `whence <path>` (below) reads it with zero eval.
    # indexable-inc/index#2418.
    provenanceModule
    mutableJsonModule
    tmuxModule
  ];

  assertions = [
    {
      assertion = cfg.packages.ix != null;
      message = "users.andrewgazelka.packages.ix must be set for the workstation profile.";
    }
    {
      assertion = cfg.packages.mercuryCli != null;
      message = "users.andrewgazelka.packages.mercuryCli must be set for the workstation profile.";
    }
    {
      assertion = cfg.packages.typenix != null;
      message = "users.andrewgazelka.packages.typenix must be set for the workstation profile.";
    }
    {
      assertion = cfg.paths.vscodeIslands != null;
      message = "users.andrewgazelka.paths.vscodeIslands must be set for the workstation profile.";
    }
    {
      assertion = cfg.sshSigningPublicKey != "";
      message = "users.andrewgazelka.sshSigningPublicKey must be set for the workstation profile.";
    }
  ];

  provenance = {
    enable = true;
    # This config repo's checkout rev, stamped on every manifest entry so
    # `whence` can say which commit defined a file (null while dirty).
    rev = cfg.configurationRevision;
  };

  news.display = "silent";

  home.sessionPath = [
    "$HOME/Library/Application Support/JetBrains/Toolbox/scripts"
    "$HOME/.cargo/bin"
    "$HOME/.bun/bin"
    "$HOME/.local/share/npm/bin"
    "$HOME/.local/share/pnpm/bin"
    "$HOME/Projects/tools/target/release"
    "/opt/homebrew/bin"
    "/opt/homebrew/sbin"
  ];

  home.sessionVariables =
    {
      EZA_ICONS = "auto";
      HOMEBREW_NO_ENV_HINTS = "1";
      LESS = "-R";
      MOOR = "--wrap --quit-if-one-screen";
      NIX_PATH = "darwin-config=${config.home.homeDirectory}/.config/nix/flake.nix:nixpkgs=flake:nixpkgs";
      NIX_PRIVATE_CONFIG_DIR = cfg.paths.privateConfigDirectory;
      PAGER = "moor";
      PKG_CONFIG = "${pkgs.pkg-config}/bin/pkg-config";
      PYTEST_XDIST_WORKER_COUNT = "auto";
      RUST_BACKTRACE = "full";

      # Bind the ix-mcp kernel's dashboard / data API to loopback. This is a
      # single-user box, and without IX_MCP_HOST the server falls back to the
      # tailnet (`bind_host = IX_MCP_HOST or <tailscale-ip> or 127.0.0.1`),
      # exposing the read-only dashboard to every tailnet peer. Setting it at the
      # device level (inherited by claude/codex and thus every ix-mcp they spawn,
      # plus any manual `ix-mcp serve`) keeps the policy in one place and the two
      # agents identical, instead of a per-server env override on each.
      IX_MCP_HOST = "127.0.0.1";

      # Skim configuration
      SKIM_CTRL_T_COMMAND = "fd --type f --hidden --follow";

      # fzf environment variables
      FZF_DEFAULT_COMMAND = "fd --type f --hidden --follow --exclude .git";
      FZF_CTRL_T_COMMAND = "fd --type f --hidden --follow --exclude .git";
      FZF_ALT_C_COMMAND = "fd --type d --hidden --follow --exclude .git";

      # Difftastic environment variables
      DFT_BACKGROUND = "dark";
      DFT_DISPLAY = "side-by-side";
      DFT_TAB_WIDTH = "4";
      DFT_SYNTAX_HIGHLIGHT = "on";
      DFT_IGNORE_COMMENTS = "false";
      DFT_SORT_PATHS = "true";

      # PyO3/Clippy fix
      PYO3_PYTHON = "${pkgs.python312}/bin/python3";

      # Claude Code feature toggles (verified against the 2.1.168 binary):
      #  - ENABLE_TOOL_SEARCH=false → sb8() returns "standard": tool search off, so
      #    every MCP tool is loaded eagerly and visible, never deferred behind a
      #    ToolSearch fetch (default would defer once MCP defs cross a char cap).
      #  - CLAUDE_CODE_DISABLE_CRON=1 → drops the scheduling/loop tools
      #    (CronCreate/CronDelete/CronList).
      # Agent teams follow the index claude-code wrapper's env_defaults
      # (index#1786 bakes CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1); the context
      # window is clamped to ~300K via claudeSettings.env above (and index#2167
      # bakes the same into the wrapper). No per-machine override for either
      # here.
      ENABLE_TOOL_SEARCH = "false";
      CLAUDE_CODE_DISABLE_CRON = "1";
    }
    // lib.optionalAttrs pkgs.stdenv.isDarwin {
      BROWSER = "open"; # macOS `open`; meaningless on the headless VM
      ITERM2_SQUELCH_MARK = 1;
      COMPOSE_BAKE = "true";
    };

  # Cross-platform packages
  # This intentionally remains a package scope: qualifying hundreds of plain
  # nixpkgs leaves makes the inventory harder to audit without improving its
  # dependency boundary. Non-nixpkgs packages stay visibly qualified below.
  # astlog-ignore: no-with-pkgs
  home.packages = with pkgs;
    [
      # Shell and terminal
      # claude-code is installed by the programs.claude-code module below (as
      # finalPackage); listing it here too would just double-add the same drv.
      # codex is likewise installed by programs.codex below (home.packages =
      # [ cfg.package ]); not listed here to avoid double-adding the same drv.
      indexPkgs.mcp # `ix-mcp`: index MCP server (search_semantic/search_grep, python_*); pinned via the `index` flake input, bump with `nix flake update index`
      indexPkgs.git-log-pretty # `git-log-pretty`: pretty ahead-of-main git log; same pinned `index` input
      indexPkgs.ast-merge # `ast-merge`: the index repo's .gitattributes merge driver for *.rs; without it on PATH a rebase outside the dev shell records markerless "ours" conflicts (silent data loss); same pinned `index` input
      indexPkgs.mynoise # `mynoise`: play myNoise.net generators (`mynoise RAIN`, `--list`); same pinned `index` input
      indexPkgs.htmlpage # `htmlpage`: render one-file TSX reports to self-contained HTML and open them; same pinned `index` input
      indexPkgs.whence # `whence <path>`: deployed file -> defining nix file:line, read from the generation's provenance.json manifest; same pinned `index` input
      cfg.packages.ix # `ix`: fleet/VM CLI (ix up/shell/ls/snapshot); pinned via the `ix` flake input, bump with `nix flake update ix`
      (callPackage (configRoot + "/home/ssh-hosts") {}) # `ssh-hosts`: list SSH aliases + recent ssh targets; backs the ssh-hosts Claude skill
      # fish: installed by programs.fish below (package = fishNoTests), not here
      # starship: installed by programs.starship (configured below)
      zellij # terminal multiplexer (tmux alternative, friendlier UI, pane layouts)
      indexPkgs.tmux # tmux wrapped with index's fleet defaults (truecolor + Claude Code 24-bit escape hatch via CLAUDE_CODE_TMUX_TRUECOLOR); sources personal ~/.tmux.conf last. Pinned via the `index` input.
      atuin # SQLite-backed shell history with fuzzy search + sync
      zoxide # smarter `cd` that ranks dirs by frecency (`z foo`)
      # iamb  # broken: compile error in nixpkgs (Matrix TUI client)

      # Core utilities
      bat # `cat` with syntax highlighting + paging
      eza # modern `ls` (icons, git status, tree view)
      fd # fast `find` alternative with sane defaults
      mgrep # semantic/natural-language code search (mosaic)
      ripgrep # fast recursive `grep` (`rg`)
      ripgrep-all # `rga` — ripgrep through PDFs, archives, sqlite, docx, etc.
      fzf # interactive fuzzy finder for any line-oriented input
      skim # Rust-native fzf alternative (`sk`)
      jq # JSON query/transform language
      yazi # TUI file manager with previews
      duf # `df` with colored bars, per-mount summary
      dust # `du` rewritten in Rust, tree view of disk usage
      dua # interactive disk-usage analyzer (`dua i`)
      pstree # process tree visualization
      htop # interactive process viewer
      indexPkgs.btop # btop fork with macOS process IO/s sorting; pinned via the `index` flake input
      tree # directory tree printer
      coreutils # GNU core utilities (ls, cp, mv, cat, etc. — GNU versions)
      moreutils # extra Unix tools: sponge, ts, vipe, parallel, ifne
      rsync # incremental file transfer / sync
      wget # non-interactive HTTP downloader
      curl # HTTP/URL transfer tool (Swiss army knife of HTTP)
      yt-dlp-light # YouTube + 1000+ sites video downloader (light variant skips heavy deps)
      socat # bidirectional byte stream relay (sockets, files, pipes, TLS)
      mosh # low-latency SSH replacement; reuses ssh-config for auth, then UDP 60000-61000
      # inetutilsForPlatform # telnet (ping/ping6/traceroute stripped on darwin; macOS ships setuid ones in /sbin)
      websocat # `curl` for WebSockets

      # Git and version control
      git # the version control system itself
      git-lfs # large-file storage extension for git
      gitoxide # pure-Rust git implementation (`gix`, used by the `gix clone` helper)
      libgit2 # C library implementing git core methods (linked against by other tools)
      gh # GitHub CLI (PRs, issues, gists, auth, runs)
      tea # Forgejo/Gitea CLI (pull requests, issues, releases)
      jujutsu # `jj` — Git-compatible VCS with first-class branches/operations
      # jj-starship  # slow to build from source (starship segment for jj repos)
      lazygit # TUI for git (stage, commit, branch, rebase visually)
      delta # syntax-highlighted git diff/blame pager
      difftastic # tree-sitter-aware structural diff (`difft`)
      mergiraf # syntax-aware merge driver to reduce conflicts
      # `git wt-submodules`: init a worktree's submodules by borrowing the main
      # clone's already-downloaded objects (--reference) instead of re-cloning
      # each over the network. See scripts/git-wt-submodules.sh.
      (ix.writeBashApplication pkgs {
        name = "git-wt-submodules";
        runtimeInputs = [git];
        text = builtins.readFile (configRoot + "/scripts/git-wt-submodules.sh");
      })

      # Development - Rust
      # cargo-audit
      # cargo-binstall
      # cargo-bloat
      # cargo-deny
      # cargo-expand
      # cargo-fuzz
      # cargo-insta
      # cargo-machete
      # cargo-modules
      # cargo-outdated
      # cargo-shear

      # Development - Node/JS
      nodejs_24 # Node.js runtime, v24 LTS line
      pnpm # fast, disk-efficient Node package manager
      prettier # opinionated code formatter (JS/TS/JSON/MD/etc.)
      typescript-language-server # LSP server for TypeScript/JavaScript
      bun # JS runtime + bundler + package manager (Zig-based)

      # Development - Python
      python313 # CPython 3.13 interpreter
      fonttools # read/write/convert font files (TTF/OTF/WOFF)
      genson # generate JSON Schema from sample JSON
      shodan # Shodan API client (internet-asset search)
      termgraph # ASCII bar/calendar charts in the terminal
      pyright # static type checker / LSP for Python
      pyupgrade # auto-upgrade Python syntax to newer versions
      ruff # fast Python linter + formatter (Rust)
      uv # ultra-fast Python package manager / venv tool

      # Development - Go/Zig
      go # Go compiler and toolchain
      zig # Zig compiler (also doubles as a `cc` cross-compiler)

      # Development - BEAM (Erlang/Gleam)
      gleam # statically-typed functional language on the BEAM VM
      beam28Packages.erlang # Erlang/OTP runtime
      beam28Packages.rebar3 # Erlang build tool / package manager

      # Development - Build tools
      cmake # cross-platform build system generator
      pkg-config # query compiler/linker flags for installed libs
      bazel_8 # Google's hermetic, parallel build system (v8.x)
      # watchman  # commented 2026-05-22: upstream folly test (UninitializedMemoryHacksTest) fails to compile under current clang, blocking rebuilds. Re-enable when nixpkgs bumps folly.
      just # command runner (`justfile` — like make without the gotchas)
      capnproto # Cap'n Proto IDL + RPC (faster Protobuf alternative)
      capnproto-rust # Cap'n Proto Rust codegen (`capnpc-rust`)
      protobuf # Protocol Buffers compiler (`protoc`)

      # Development - Debugging
      gdb # GNU debugger. NOTE on Apple Silicon: gdb loads symbols and sets breakpoints but CANNOT run/attach to native arm64 Mach-O processes (`run` => "Don't know how to run"; upstream has no arm64 darwin-nat target, codesigning does not fix it). Use the system `lldb` for live arm64 debugging. gdb here is for remote/cross targets: gdbserver in a Linux ix VM, QEMU gdbstub, core files, and static symbol inspection.

      # Editors
      neovim # Vim fork with Lua scripting and modern plugin ecosystem
      tree-sitter # incremental parser library + CLI (powers syntax highlighting)

      # Cloud and infrastructure
      awscli2 # AWS CLI v2
      # azure-cli # Microsoft Azure CLI (`az`); disabled 2026-07-01: aarch64-darwin output was absent from cache.nixos.org and rebuilt locally for ~50m during a routine flake update. Use `nix run nixpkgs#azure-cli -- ...` when needed.
      gws # Google Workspace CLI (official, from googleworkspace/cli)
      (ix.writeBashApplication pkgs {
        name = "gws-ix";
        text = ''
          export GOOGLE_WORKSPACE_CLI_CONFIG_DIR="$HOME/.config/gws-ix"
          exec ${pkgs.gws}/bin/gws "$@"
        '';
      })
      (ix.writeBashApplication pkgs {
        name = "gws-personal";
        text = ''
          export GOOGLE_WORKSPACE_CLI_CONFIG_DIR="$HOME/.config/gws-personal"
          exec ${pkgs.gws}/bin/gws "$@"
        '';
      })
      google-cloud-sdk # gcloud / gsutil / bq for GCP
      pulumi # infrastructure-as-code in real programming languages
      pulumiPackages.pulumi-nodejs # Pulumi Node.js language host
      sshpass # provide password to ssh non-interactively (CI/automation)
      cloudflared # Cloudflare Tunnel client (`cloudflared tunnel`)
      # wrangler # Cloudflare Workers/Pages CLI — temporarily removed: 4.93.0
      # fails to build on nixpkgs-unstable (pnpm 10.34.0 trips Node ESM-loader
      # EBADF regression, nodejs/node#62012, in wrangler's generate-json-schema
      # step). Uncomment once unstable's wrangler builds again. `nix run
      # nixpkgs#wrangler` still works from an older registry pin meanwhile.
      runpodctl # RunPod GPU cloud CLI
      # Kubernetes
      kind # local Kubernetes clusters in Docker (Kind = K8s IN Docker)

      # Databases
      postgresql_18 # Postgres 18 server + client (`psql`, `pg_dump`, etc.)
      libpq # Postgres client C library (used by tools/libraries that link libpq)
      libpq.dev # libpq headers/pkg-config files for compiling against libpq
      mysql84 # MySQL 8.4 server + client
      redis # Redis server + `redis-cli`
      diesel-cli # Rust ORM/migration CLI for Diesel
      pscale # PlanetScale CLI (branch/connect/deploy)
      usql # universal SQL REPL — connects to any of 25+ DBs
      pgcli # `psql` with autocomplete and syntax highlighting
      # mycli  # broken in nixpkgs (would be the mysql equivalent of pgcli)

      # Containers
      dive # explore docker image layers + show wasted space
      ctop # `top` for containers (per-container CPU/mem/net/io)

      # Network/Security tools
      # wireguard-tools                    # WireGuard VPN tooling (wg, wg-quick)
      mtr # traceroute + ping in one continuous report
      doggo # modern `dig` (DNS lookups, multiple resolvers)
      grpcurl # `curl` for gRPC services (reflection-aware)
      mitmproxy # interactive HTTPS proxy for inspecting/modifying traffic
      scraper # scrape/export git commits, Discord, Matrix, GitHub (indexable-inc/scraper flake)
      # rustup                             # Rust toolchain installer (managed elsewhere — usually project-local)
      speedtest-cli # Ookla speedtest (Python client)
      speedtest-go # Go-based speedtest (alternative implementation)
      swaks # SMTP "Swiss Army Knife": scripted EHLO/STARTTLS/AUTH/MAIL FROM testing
      # cloudflare-speed-cli  # broken on macOS: test expects Linux lo interface

      wasm-pack # build + package Rust crates as WebAssembly

      termdown # countdown timer / stopwatch in the terminal
      peaclock # minimal terminal clock + timer

      # cargo-nextest                      # faster Rust test runner

      snitch # personal task / scratchpad CLI
      ovhcloud-cli # OVHcloud provider CLI

      opentimestamps-client # create/verify Bitcoin-blockchain timestamps for files
      poppler-utils # PDF tools: pdftotext, pdfinfo, pdfimages, etc.

      skopeo # work with container images & registries without a daemon

      # Profiling
      # tracy                              # high-resolution profiler (GUI)

      # Media
      ffmpeg # audio/video transcoding & manipulation Swiss army knife
      imagemagick # image conversion / processing (`convert`, `magick`)
      sox # audio processing CLI (transform, filter, mix)
      # yt-dlp  # slow to build from source — using yt-dlp-light above instead
      viu # show images directly in the terminal

      # Document processing
      typst # modern typesetting system (Rust, sane LaTeX alternative)
      tinymist # Typst LSP server (used by editors for completion/preview)
      tectonic # self-contained, modern LaTeX engine
      pdfgrep # grep through PDF text
      newcomputermodern # Computer Modern font family (LaTeX classic, modernized)

      # Code analysis
      ast-grep # structural search/replace by AST (multiple languages)
      onefetch # repo info card in the terminal (lang stats, authors, deps)
      scc # fast SLOC counter with COCOMO estimates
      tokei # fast SLOC counter by language
      dotslash # Meta's cross-platform single-file executable launcher
      codesearch # Google's indexed regex code search (`csearch`, `cindex`)
      codeowners # parse/test GitHub CODEOWNERS files

      # Nix tools
      # nixos-rebuild: present on the vfkit guest natively, but NOT on hydra (macOS
      # ships only darwin-rebuild). Installing it here lets hydra deploy the
      # headless NixOS guest from this one flake instead of a second clone. The
      # guest runs sshd (hosts/vm); reach it on its vmnet NAT IP, e.g.
      #   nixos-rebuild switch --flake ~/.config/nix#vm \
      #     --target-host andrewgazelka@"$(vm-ssh -- true; ...)" --use-remote-sudo
      # In practice: `vm-ssh` resolves the IP from /var/db/dhcpd_leases, so
      #   nixos-rebuild switch --flake ~/.config/nix#vm --target-host andrewgazelka@<ip>
      # or just rebuild from inside the guest. Key-only login; the key is in
      # ssh-keys/andrewgazelka.pub.
      nixos-rebuild
      nil # Nix language server (static analysis)
      nixd # Nix language server (evaluation-based, better flake/nixpkgs completion)
      # Experimental: TypeScript-grade type checker LSP for Nix (tsgo fork).
      # https://github.com/ryanrasti/typenix — invoked via typenix-lsp wrapper.
      cfg.packages.typenix
      (ix.writeBashApplication pkgs {
        name = "typenix-lsp";
        text = ''
          exec ${cfg.packages.typenix}/bin/typenix --lsp --stdio "$@"
        '';
      })
      # Experimental: MLsub/SimpleSub type checker LSP for Nix (Nix-native).
      # https://github.com/JRMurr/tix — Cursor/Zed point at tix-lsp.
      # inputs.tix.packages.${pkgs.stdenv.hostPlatform.system}.default
      # (pkgs.writeShellScriptBin "tix-lsp" ''
      #   exec ${inputs.tix.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/tix lsp "$@"
      # '')
      nix-index # find which package provides a given file/binary (`nix-locate`)
      nvd # diff Nix store paths and profile generations after rebuilds
      indexPkgs.nix-output-monitor # `nom` patched so nix-derivation parses content-addressed derivations (index repo builds them); upstream nixpkgs nom spams DerivationParseError. Bump with `nix flake update index`
      # indexable-inc/index#1711: eval needs the x86_64-linux IFD output
      # (cargo-units.nix); when cache.ix.dev lacks it, copy it from the fleet:
      # `nix copy --no-check-sigs --from ssh-ng://vin-compute-1 <path from the eval error>`
      indexPkgs.nix-web-monitor # `nwm` web build monitor on :7532
      nix-tree # interactive TUI to browse a store path's dependency graph (`nix-tree ./result`); "Added size" column = a path's unique cost on top of everything else
      nix-du # graph which GC-roots to delete to actually free space (`nix-du -s 500MB | dot -Tsvg`); accounts shared-vs-unique via .links
      graphviz # `dot` etc.; renders nix-du's dependency graph to svg/png
      nixfmt # official Nix formatter (RFC-101)
      nixpkgs-fmt # alternative Nix formatter (nixpkgs style)
      cachix # Nix binary cache as a service (push/pull from cachix.org)
      direnv # per-directory env loader (auto-loads .envrc / shell.nix on cd)
      # devbox             # per-project dev shells built on Nix, with a simpler UX
      fh # FlakeHub CLI (search/install/publish flakes via flakehub.com)

      # Misc utilities
      agent-browser # headless Chrome CLI for AI agents (vercel-labs/agent-browser) — ref-based snapshots, ~hundreds of tokens vs DOM
      _1password-cli # 1Password CLI (`op`) for secrets / SSH agent integration
      rbw # unofficial Bitwarden/Vaultwarden CLI; macOS config is in profiles/darwin-home.nix
      abduco # lightweight session manager (attach/detach background processes)
      act # run GitHub Actions workflows locally (Docker-based)
      ansifilter # strip or convert ANSI escape sequences (to HTML, LaTeX, plain)
      bats # Bash Automated Testing System (TAP-based shell test framework)
      crane # OCI image manipulation (push/pull/inspect without a daemon)
      dasel # query/modify JSON/YAML/TOML/XML/CSV with one tool
      # dotenvx  # broken in nixpkgs (modern dotenv with encryption)
      dvtm # tiling window manager for the terminal (dwm-like)
      glfw # OpenGL/Vulkan windowing & input library (linked by graphics tools)
      glow # render markdown in the terminal
      hyperfine # statistical command-line benchmarking
      mutagen # fast bidirectional file/folder sync (great over SSH)
      openssl # TLS/crypto library + `openssl` CLI
      pmtiles # Extract / inspect / serve Protomaps PMTiles archives
      process-compose # docker-compose-style orchestrator for plain processes
      quicktype # generate type definitions from JSON samples (many target languages)
      sd # `sed` alternative with regex-by-default, intuitive syntax
      sentry-cli # Sentry release / sourcemap upload CLI
      stripe-cli # Stripe API testing, webhook forwarding, log tailing
      # tabiew          # expensive Rust/Polars build on macOS (TUI for CSV/Parquet/etc.)
      taplo # TOML formatter / linter / LSP
      termdown # countdown timer / stopwatch (duplicate also listed above)
      twurl # `curl` with built-in Twitter/X OAuth signing
      ugrep # fast grep with PDF/zip/tar/JSON support
      unar # universal unarchiver (rar/7z/zip/tar/etc. via The Unarchiver)
      viddy # modern `watch` with diff highlighting and time-travel
      vivid # LS_COLORS theme generator (used by `eza`/`ls`)
      wabt # WebAssembly Binary Toolkit (`wasm2wat`, `wat2wasm`, `wasm-objdump`)
      wasmtime # standalone WebAssembly runtime
      watch # rerun a command periodically and show output
      nushellWithNativeClip # structured-data shell with native `clip copy` / `clip paste`
      xonsh # Python-powered shell
      zsh # Z shell (alt to bash, system default on macOS)
      zstd # Zstandard fast compression (`zstd`, used by many tools)
      indexPkgs.yc # YC CLI: search Bookface + chat with the YC Agent; needs Bookface login
    ]
    ++ lib.optionals pkgs.stdenv.isDarwin [
      # macOS-only packages
      # aerospace is provided declaratively by programs.aerospace below.
      alacritty # GPU-accelerated terminal emulator
      # clonefile                                       # CoW file copies on APFS
      create-dmg # build .dmg installer disks from a folder
      mas # Mac App Store CLI (search/install/upgrade)
      macpm # macOS package manager wrapper / unified front-end
      notify # `notify "msg"` — short spoken status updates via macOS `say` (serialized; from andrewgazelka/notify flake)
      protoc-gen-swift # Swift codegen plugin for protoc
      # sif                                             # Singularity Image Format tooling
      swiftformat # Swift source code formatter
      vengi-tools # voxel/mesh conversion toolkit; `vengi-voxconvert` supports binvox without x86_64-darwin nixpkgs
      # wezterm # GPU terminal emulator; disabled 2026-07-01: aarch64-darwin output was absent from cache.nixos.org and rebuilt locally for ~46m during a routine flake update. Use Alacritty or `nix run nixpkgs#wezterm -- ...` when needed.
      cfg.packages.mercuryCli # Mercury CLI (custom flake input)
      indexPkgs.elevenlabs-say # ElevenLabs say-style TTS CLI (-r/-v, streaming); key via ELEVENLABS_API_KEY
      # vfkit guest helpers intentionally not installed: they force the
      # aarch64-linux microvm system into every Home Manager switch, so a stale
      # or stopped VM remote builder breaks unrelated macOS profile updates.
    ];

  # ============================================
  # Cross-platform config symlinks (.config/)
  # ============================================

  # AeroSpace (macOS tiling WM), declaratively. Keybindings come from the shared
  # hosts/wm-keybinds.nix (portable keymap); macOS-only extras
  # and window rules stay here. launchd.enable lets home-manager own startup
  # (the module forbids start-at-login without it).
  programs.aerospace = lib.mkIf pkgs.stdenv.isDarwin {
    enable = true;
    launchd.enable = true;
    settings = {
      enable-normalization-flatten-containers = true;
      enable-normalization-opposite-orientation-for-nested-containers = false;
      accordion-padding = 100;

      workspace-to-monitor-force-assignment = {
        "1" = "main";
        "2" = "main";
        "3" = "main";
        "4" = "main";
        "5" = "main";
        "6" = "main";
        "7" = "main";
        "8" = "main";
        "9" = "secondary";
      };

      # NOTE: cmd-1/2/3 repo shortcuts live in Ghostty's own config (native
      # `text` keybind that types `cd <repo>` into the focused terminal).
      # AeroSpace must NOT bind cmd-1/2/3, or its global hotkey tap would
      # swallow them before Ghostty ever sees the keys.
      mode.main.binding =
        wm.aerospaceBindings
        // {
          # macOS-only extras (no clean sway equivalent), preserved verbatim.
          "alt-tab" = "workspace-back-and-forth";
          "alt-backtick" = "focus-back-and-forth";
          "alt-a" = "layout tiles accordion";
          "alt-shift-e" = "balance-sizes";
          "alt-shift-minus" = "resize smart -50";
          "alt-shift-equal" = "resize smart +50";
          "ctrl-alt-shift-minus" = "resize smart-opposite -50";
          "ctrl-alt-shift-equal" = "resize smart-opposite +50";
          "alt-shift-h" = "join-with left";
          "alt-shift-j" = "join-with down";
          "alt-shift-k" = "join-with up";
          "alt-shift-l" = "join-with right";
        };

      on-window-detected = [
        {
          "if"."app-id" = "com.microsoft.Excel";
          run = "layout tiling";
        }
        {
          "if"."app-id" = "com.paulsolt.SuperEasyTimerMac";
          run = "layout floating";
        }
        {
          "if"."app-id" = "com.apple.FaceTime";
          run = "layout floating";
        }
        {
          "if"."app-id" = "notion.id";
          run = "layout tiling";
        }
        {
          "if"."app-id" = "com.linear";
          run = "layout tiling";
        }
        {
          "if"."app-id" = "com.obsproject.obs-studio";
          run = "layout floating";
        }
        {
          # ix-windows renders each live MCP resource as a floating, blurred,
          # always-on-top overlay card (transparent tao+wry, no bundle id). Match
          # on app name and force floating so AeroSpace leaves the overlay alone
          # instead of tiling it into the grid.
          "if"."app-name-regex-substring" = "ix-windows";
          run = "layout floating";
        }
      ];
    };
  };

  # Emacs (~/.emacs.d is auto-created on first launch and takes precedence over XDG)
  home.file.".emacs.d/init.el".source = repoFile "emacs/init.el";

  # Starship is configured natively below via `programs.starship` (settings
  # from home/starship.nix), so no out-of-store symlink here.

  # Btop
  home.file.".config/btop/btop.conf".source =
    renderBtop "andrewgazelka-btop.conf" (import (configRoot + "/btop"));

  # Process-compose
  home.file.".config/process-compose/process-compose.yaml".source =
    yamlFormat.generate "andrewgazelka-process-compose.yaml" yamlStructured.process-compose-process-compose;
  home.file.".config/process-compose/theme.yaml".source =
    yamlFormat.generate "andrewgazelka-process-compose-theme.yaml" yamlStructured.process-compose-theme;

  # Direnv
  home.file.".config/direnv/direnv.toml".source = renderStructured "direnv-direnv";

  # Atuin
  # iamb (Matrix client). The macOS app reads ~/Library/Application Support/iamb;
  # the XDG ~/.config/iamb covers Linux. Both render from the same Nix value.
  home.file.".config/iamb/config.toml".source = renderStructured "iamb-config";
  home.file."Library/Application Support/iamb/config.toml".source =
    lib.mkIf pkgs.stdenv.isDarwin (renderStructured "iamb-config");

  # Git
  home.file.".config/git/attributes".source =
    renderLines "andrewgazelka-gitattributes" (import (configRoot + "/git/attributes.nix"));

  # Zed (the macOS-only icon overrides live under ~/Library/Application Support).
  home.file.".config/zed/keymap.json".source = jsonFormat.generate "andrewgazelka-zed-keymap.json" (
    import (configRoot + "/zed/keymap.nix")
  );
  home.file.".config/zed/settings.json".source =
    jsonFormat.generate "andrewgazelka-zed-settings.json"
    (import (configRoot + "/zed/settings.nix"));
  home.file."Library/Application Support/Zed/extensions/installed/jetbrains-new-ui-icons/icons/andrew-folder-test-green.svg" = lib.mkIf pkgs.stdenv.isDarwin {
    source = repoFile "zed/icons/andrew-folder-test-green.svg";
  };
  home.file."Library/Application Support/Zed/extensions/installed/jetbrains-new-ui-icons/icons/andrew-folder-test-green-dark.svg" = lib.mkIf pkgs.stdenv.isDarwin {
    source = repoFile "zed/icons/andrew-folder-test-green-dark.svg";
  };

  home.activation.patchZedNamedFolderIcons = lib.mkIf pkgs.stdenv.isDarwin (
    lib.hm.dag.entryAfter ["linkGeneration"] ''
      theme_file="$HOME/Library/Application Support/Zed/extensions/installed/jetbrains-new-ui-icons/icon_themes/jetbrains-new-ui-icons-theme.json"

      if [[ -f "$theme_file" ]]; then
        tmp="$(mktemp)"
        ${pkgs.jq}/bin/jq '
          def named_directory_icons($path): {
            "test": {
              "collapsed": $path,
              "expanded": $path
            },
            "tests": {
              "collapsed": $path,
              "expanded": $path
            }
          };

          .["$schema"] = "https://zed.dev/schema/icon_themes/v0.3.0.json"
          | .themes |= map(
              if (.name | contains("(Dark)")) then
                .named_directory_icons = ((.named_directory_icons // {}) + named_directory_icons("./icons/andrew-folder-test-green-dark.svg"))
              else
                .named_directory_icons = ((.named_directory_icons // {}) + named_directory_icons("./icons/andrew-folder-test-green.svg"))
              end
            )
        ' "$theme_file" > "$tmp"
        mv "$tmp" "$theme_file"
      else
        echo "Skipping Zed named folder icon patch; JetBrains New UI Icons is not installed."
      fi
    ''
  );

  # k9s
  home.file.".config/k9s/config.yaml".source =
    yamlFormat.generate "andrewgazelka-k9s.yaml" yamlStructured.k9s-config;
  home.file.".config/k9s/skins/main.yaml".source =
    yamlFormat.generate "andrewgazelka-k9s-main-skin.yaml" yamlStructured.k9s-skins-main;

  # jj (Jujutsu)
  home.file.".config/jj/config.toml".source = renderStructured "jj-config";

  # Tap
  home.file.".config/tap/config.toml".source = renderStructured "tap-config";

  # Ghostty themes and shaders (cross-platform location)
  home.file.".config/ghostty/themes".source = repoFile "ghostty/themes";
  home.file.".config/ghostty/shaders".source = repoFile "ghostty/shaders";

  # Alacritty
  home.file.".config/alacritty/alacritty.toml".source = renderStructured "alacritty-alacritty";

  # Kitty
  home.file.".config/kitty/kitty.conf".source =
    renderKitty "andrewgazelka-kitty.conf" (import (configRoot + "/kitty"));

  # WezTerm
  home.file.".config/wezterm/wezterm.lua".source = repoFile "wezterm/wezterm.lua";

  # ============================================
  # Home directory dotfiles
  # ============================================

  # Cargo
  home.file.".cargo/config.toml".source = renderStructured "cargo-config";

  # Git
  home.file.".gitignore_global".source =
    renderLines "andrewgazelka-gitignore" (import (configRoot + "/git/ignore.nix"));

  # SSH config is generated by programs.ssh below. Only keep the controlmasters
  # directory marker here; programs.ssh writes ~/.ssh/config itself.
  home.file.".ssh/controlmasters/.keep".text = ""; # ensure directory exists for ControlPath

  # known_hosts is split in two (see UserKnownHostsFile in programs.ssh below):
  # stable fleet/infra keys are supplied by the private consumer and compiled
  # into the Nix store; ssh writes runtime-accepted hosts to the
  # mutable, untracked ~/.ssh/known_hosts. Nothing under ssh/ is tracked here.

  # Bash and Zsh are generated by Home Manager; tool integrations are owned by
  # their respective program modules rather than duplicated shell snippets.
  programs.zsh.enable = true;

  programs.tmux.structured = (import (configRoot + "/tmux")) // {enable = true;};

  # NPM — contains a live registry _authToken, so workstation-only (out-of-store);
  # never bake it into the world-readable nix store on another host.
  home.file.".npmrc" = lib.mkIf pkgs.stdenv.isDarwin {
    source = config.lib.file.mkOutOfStoreSymlink "${configDir}/npmrc";
  };

  # IdeaVim
  home.file.".ideavimrc".source =
    renderLines "andrewgazelka-ideavimrc" (import (configRoot + "/ideavim"));

  # LLDB
  home.file.".lldbinit".source =
    renderLines "andrewgazelka-lldbinit" (import (configRoot + "/lldb"));

  # Suppress the macOS "Last login" banner in new terminal sessions.
  home.file.".hushlogin".text = "";

  # Mitmproxy — holds the private CA key material, so workstation-only
  # (out-of-store); never bake it into the world-readable nix store.
  home.file.".mitmproxy" = lib.mkIf pkgs.stdenv.isDarwin {
    source = config.lib.file.mkOutOfStoreSymlink "${configDir}/mitmproxy";
  };

  # Claude CLI binary on PATH at a stable path. The package itself is installed
  # via programs.claude-code below (as finalPackage); this is just a fixed
  # location some tooling expects.
  home.file.".local/bin/claude" = {
    # finalPackage, not the raw claudeCode: the module wraps it with the
    # declarative MCP plugin dir (see programs.claude-code.mcpServers), so this
    # stable-path binary carries the same MCP set as the one on the home profile
    # PATH instead of silently bypassing it.
    source = "${config.programs.claude-code.finalPackage}/bin/claude";
    force = true;
  };

  # Our own Codex on PATH at a stable path, same pattern as claude above. This
  # is the MCP-injecting wrapper (see the `codex` let-binding), so the declared
  # MCP set rides every invocation.
  home.file.".local/bin/codex" = {
    source = "${codex}/bin/codex";
    force = true;
  };

  # Agents, commands, and skills are managed declaratively by the upstream
  # home-manager programs.claude-code module: each is written as an in-store
  # path under ~/.claude (no out-of-store symlink, no SessionStart hook).
  # SKILLS now ride the module's `skills` option (the shared `skillsSrc` index
  # dir), delivered bare to ~/.claude/skills/<name> and invoked as `/<skill>`
  # (no `index:` namespace): the same source Codex gets via programs.codex
  # below. This replaces the old baked `--plugin-dir` plugin.
  # settings.json / .claude.json are deliberately NOT routed through it (the
  # module only touches settings.json when `settings` is set, which it is not
  # here); app-owned writable state stays out-of-store below. CLAUDE.md is
  # generation-owned from the tracked source because the app never rewrites it.
  programs.claude-code = {
    enable = true;
    package = claudeCode;
    houseContext.enable = false;
    # All agents, BARE, sourced straight from the index repo (index's agents
    # package now holds my former personal agents too). Bare (not plugin) so
    # `subagent_type code-reviewer` keeps resolving.
    agentsDir = indexPkgs.agents;
    commandsDir = repoFile "claude/global/commands";
    # Bare skills (the shared index dir): module symlinks ~/.claude/skills, so
    # they invoke as `/<skill>`, not `/index:<skill>`. Same source as Codex.
    skills = skillsSrc;
    # MCP servers are NOT delivered here: `programs.claude-code.mcpServers` bundles
    # them into a plugin dir that Claude double-loads (plain + plugin-namespaced),
    # spawning two ix-mcp kernels per session. Instead the claude-code package
    # bakes one `--mcp-config=` flag from `agentMcpServers` above.
  };

  # Codex, via the UPSTREAM home-manager programs.codex module (sibling to
  # programs.claude-code). `package` is our index `codex` wrapper, already
  # `.override`n with settings/forcedSettings that bake as `-c` flags; we
  # deliberately leave the module's config-toml inputs UNSET so it writes NO
  # ~/.codex/config.toml. The module writes config.toml only when
  # `mergedSettings != {}`, i.e. when ANY of `settings`, `plugins`,
  # `marketplaces`, or `enableMcpIntegration` is set; none are here, so codex
  # keeps config.toml as its own mutable runtime file. (Do NOT set any of those
  # on this module: a nix-written config.toml is read-only and codex errors
  # trying to churn it.) The module installs the package on PATH, symlinks each
  # skill dir into ~/.codex/skills/<name> (bare, coexisting with unmanaged
  # skills; same `skillsSrc` as Claude), and writes AGENTS.md from `context`.
  # hooks.json is the one thing it does not do; delivered separately below.
  programs.codex = {
    enable = true;
    package = codex;
    skills = skillsSrc;
    context = repoFile "claude/global/CLAUDE.md";
  };

  # NOTE: ~/.claude/settings.json is intentionally NOT managed here. All static
  # config is declared in Nix (`claudeSettings`) and delivered through the
  # wrapper's read-only `--settings` flagSettings layer, which outranks and is
  # separate from this file. Leaving it unmanaged keeps it a real writable file
  # the CLI can churn for runtime state, with no symlink (avoids #3575/#58443).
  # OUT-OF-STORE on purpose: the keybindings UI / keybindings-help skill edit
  # this file in place, so it must stay writable.
  # CLAUDE.md is read-only and generation-owned. `force` replaces any stale
  # real file or legacy out-of-store link at the target.
  home.file.".claude/CLAUDE.md" = {
    source = repoFile "claude/global/CLAUDE.md";
    force = true;
  };
  # ~/.claude.json is entirely runtime-owned and intentionally unmanaged.

  # Authenticate Nix's GitHub API calls so `nix flake update` and `github:`
  # inputs don't hit the 60 req/hr unauthenticated rate limit (HTTP 403 "API
  # rate limit exceeded … using cached version", which silently keeps stale
  # revs). Regenerates ${configDir}/access-tokens.conf (gitignored, mode 600)
  # from the gh CLI on every switch; nix.conf `!include`s it. Best-effort: a
  # missing/unauthenticated gh just skips it rather than failing the switch.
  # Darwin-only: writes into the ${configDir} checkout, which only exists on the
  # workstation. Other hosts (the VM) authenticate Nix's GitHub calls via their
  # own system-level access-tokens.conf instead.
  home.activation.nixGithubAccessToken = lib.mkIf pkgs.stdenv.isDarwin (
    lib.hm.dag.entryAfter ["writeBoundary"] ''
      tokenFile="${configDir}/access-tokens.conf"
      if token="$(${pkgs.gh}/bin/gh auth token 2>/dev/null)" && [ -n "$token" ]; then
        ( umask 077; printf 'access-tokens = github.com=%s\n' "$token" > "$tokenFile" )
        echo "✓ wrote Nix GitHub access token → $tokenFile"
      else
        echo "⚠ gh not authenticated — skipping Nix GitHub access token (run: gh auth login)" >&2
      fi
    ''
  );

  # Purge cookies for every blocked domain on each switch, so a DNS-blocked
  # site (flake.nix `blockedHosts`, the same list that feeds /etc/hosts) is also
  # logged out. `hms` runs darwin-rebuild then this, so one switch blocks DNS
  # and clears cookies together.
  # Darwin-only: clears cookies from macOS browser stores; no-op / inapplicable
  # on a headless host.
  home.activation.clearBlockedCookies = lib.mkIf pkgs.stdenv.isDarwin (
    lib.hm.dag.entryAfter ["writeBoundary"] ''
      run ${clearBlockedCookies}/bin/clear-blocked-cookies ${lib.escapeShellArgs cfg.blockedHosts}
    ''
  );

  # Codex hooks (the one thing the programs.codex module above does NOT deliver;
  # config.toml/AGENTS.md/skills rationale lives there). Rendered from the SAME
  # declaration list as Claude's, owned by the index repo
  # (packages/agent/hooks.nix) and exposed as the codex package's
  # `passthru.hooksJson`. Delivered to ~/.codex/hooks.json, codex's
  # discovery path (it has no `-c` hooks-file pointer). The compiled
  # claude-hooks commands are absolute store paths, so a switch bakes the wiring;
  # codex's hash-pinned trust gate means a one-time `/hooks` trust after a bump.
  home.file.".codex/hooks.json" = {
    source = codexBase.passthru.hooksJson;
    force = true;
  };

  # Writable application configuration with declarative field ownership. The
  # mutable-json module preserves application-added keys while reconciling the
  # tracked declaration on activation.
  home.mutableJsonFiles = {
    claude-keybindings = {
      target = ".claude/keybindings.json";
      value = structured.claude-global-keybindings.value;
    };
    cursor-mcp = {
      target = ".cursor/mcp.json";
      value = builtins.fromJSON (
        builtins.replaceStrings
        ["/Users/andrewgazelka"]
        [config.home.homeDirectory]
        (builtins.toJSON structured.cursor-mcp.value)
      );
    };
    cursor-cli = {
      target = ".cursor/cli-config.json";
      value = structured.cursor-cli-config.value;
    };
    amp = {
      target = ".config/amp/settings.json";
      value = structured.amp-settings.value;
    };
  };

  # Cursor extensions sourced from github flake inputs. Dir name must match
  # `relativeLocation` in ~/.cursor/extensions/extensions.json (Cursor manages
  # that file on startup). Update with: nix flake update vscode-islands
  home.file.".cursor/extensions/vscode-islands" = {
    source = cfg.paths.vscodeIslands;
    force = true;
  };

  # Tracy
  home.file.".config/tracy/tracy.ini" = {
    source = iniFormat.generate "andrewgazelka-tracy.ini" (import (configRoot + "/tracy"));
    force = true;
  };

  # ============================================
  # Programs configuration
  # ============================================

  programs.home-manager.enable = true;

  # The generated Home Manager option manpage currently evaluates nixpkgs'
  # options.json derivation, which emits a string-context warning on this Nix.
  manual.manpages.enable = false;

  programs.man = {
    generateCaches = false;
  };

  programs.fish = {
    # The handwritten fish/ config was removed in fccfc44b; home-manager now
    # owns fish so every shell gets the nix PATH and hm-session-vars
    # (NIX_SSL_CERT_FILE and friends) without a system-level hook, which
    # nix-darwin only provides for zsh and bash.
    enable = true;
    # Use the same fishNoTests build the rest of the config references. On Linux
    # `pkgs.fish` runs upstream tests (different store hash) while fishNoTests
    # sets doCheck=false; if programs.fish installed plain pkgs.fish, home-manager
    # buildEnv would collide with fishNoTests ("two paths contain .../bin/fish").
    # On darwin both resolve to the same derivation, so this is a no-op there.
    package = fishNoTests;
  };

  programs.yazi = {
    enable = true;
    shellWrapperName = "y";
    settings = {
      mgr = {
        show_hidden = false;
      };
    };
  };

  programs.direnv = {
    enable = true;
    nix-direnv.enable = true;
  };

  programs.starship = {
    enable = true;
    # Store-backed, not an out-of-store symlink: both hydra and the nixos VM
    # render the identical prompt from one definition, regardless of where each
    # host's repo clone lives. nushell loads starship via its own vendor
    # autoload (it reads ~/.config/starship.toml that this writes), so the
    # module's shell integrations stay off.
    enableNushellIntegration = false;
    # Settings are pure nix (home/starship.nix): one source of truth that can
    # reference flake inputs directly, e.g. the index/ix pin ages segment.
    settings = import (configRoot + "/home/starship.nix") {
      inherit (cfg) pinTimestamps;
    };
  };

  programs.zoxide = {
    enable = true;
  };

  programs.zellij =
    {
      enable = true;
    }
    // import (configRoot + "/zellij") {
      inherit configRoot;
      inherit (pkgs) stdenvNoCC zellijPlugins;
      xdgConfigHome = config.xdg.configHome;
    };

  programs.atuin = {
    enable = true;
    settings = structured.atuin-config.value;
  };

  programs.eza = {
    enable = true;
    git = true;
    icons = "auto";
  };

  programs.nh =
    {
      enable = true;
      # nh wraps its own nom onto PATH ahead of home.packages, so it bypasses the
      # CA-patched nom from index (see the home.packages entry) and spams
      # `DerivationParseError "string"`; rewire the wrapper to the patched one.
      package = pkgs.nh.override {
        inherit (indexPkgs) nix-output-monitor;
      };
    }
    # The default flake lives at the workstation checkout path; other hosts have no
    # checkout here, so leave nh without a default flake there.
    // lib.optionalAttrs pkgs.stdenv.isDarwin {
      flake = cfg.paths.privateConfigDirectory;
    };

  xdg.configFile."bat/config".text = ''
    --pager='less -FR --mouse'
  '';

  home.file.".gitconfig".text = let
    ghCredentialHelper = [
      ""
      "!gh auth git-credential"
    ];
    gitCredentialHelpers =
      lib.mapAttrs' (host: helper: lib.nameValuePair ''credential "https://${host}"'' {inherit helper;})
      {
        "github.com" = ghCredentialHelper;
        "git.ix.dev" = ghCredentialHelper;
        "gist.github.com" = ghCredentialHelper;
      };
  in
    lib.generators.toGitINI (
      {
        user = {
          name = "Andrew Gazelka";
          email = "andrew.gazelka@gmail.com";
          # Private-key PATH, not the literal pubkey: git's default ssh signer
          # (`ssh-keygen -Y sign -f <this>`) reads the unencrypted on-disk key
          # directly, so signing never blocks on the 1Password app being
          # unlocked (agents repeatedly wedged on that; see auto-memory
          # op-ssh-sign-locked / git-1password-signing-noninteractive).
          signingkey = "${config.home.homeDirectory}/.ssh/id_ed25519";
        };
        commit = {
          # Sign on the workstation; off on hosts that may lack the key file
          # so commits don't fail.
          gpgsign = pkgs.stdenv.isDarwin;
          cleanup = "scissors";
          verbose = true;
        };
        alias = {
          lg = "log --pretty=format:'%s %C(dim)%h%C(reset)'";
          cleanup = ''!git fetch --prune && git branch -vv | grep ": gone]" | grep -v "\\*" | awk "{print \\$1}" | xargs -r git branch -d'';
          dft = "diff --ext-diff";
          show = "show --ext-diff";
          log = "log -p --ext-diff";
          sl = "branchless smartlog";
          sw = "branchless switch";
          sync = "!git pull --rebase && git push";
          split = ''!f() { old=$(git branch --show-current); if test -z "$old"; then echo "git split: detached HEAD is not supported" >&2; return 1; fi; if test -z "$1"; then echo "usage: git split <new-branch> [base-ref]" >&2; return 1; fi; new=$1; if test -n "$2"; then base_ref=$2; else base_ref='@{u}'; fi; if test -n "$(git status --porcelain)"; then echo "git split: commit or stash worktree changes first" >&2; return 1; fi; if git show-ref --verify --quiet "refs/heads/$new"; then echo "git split: branch '$new' already exists" >&2; return 1; fi; base=$(git rev-parse --verify --quiet "$base_ref^{commit}") || { echo "git split: cannot resolve base '$base_ref'" >&2; return 1; }; git branch "$new" HEAD || return 1; git reset --hard "$base" || return 1; git switch "$new"; }; f "$@"'';
        };
        gpg.format = "ssh";
        # Lets `git log --show-signature` verify locally. The pubkey comes
        # from the shared ssh-keys list (single source; the entry labeled
        # "signing") rather than a second inline copy that would drift on
        # rotation. namespaces="git" keeps the trust scoped to commit
        # signatures, not arbitrary `ssh-keygen -Y` namespaces.
        "gpg \"ssh\"".allowedSignersFile = let
          signingPub = cfg.sshSigningPublicKey;
          keyFields = lib.concatStringsSep " " (lib.take 2 (lib.splitString " " signingPub));
        in
          pkgs.writeText "git-allowed-signers" ''
            andrew.gazelka@gmail.com,andrew@ix.dev namespaces="git" ${keyFields}
          '';
        gc = {
          writeCommitGraph = true;
          auto = 256;
          autopacklimit = 10;
          autodetach = true;
          pruneExpire = "2.weeks.ago";
          reflogExpire = "90.days.ago";
          reflogExpireUnreachable = "30.days.ago";
        };
        pack = {
          packSizeLimit = "2g";
          compression = 1;
          depth = 50;
          threads = 0;
          useBitmaps = true;
          useSparse = true;
          window = 10;
          windowMemory = "256m";
          writeReverseIndex = true;
        };
        clone = {
          filterSubmodules = true;
          defaultRemoteName = "origin";
        };
        format.pretty = "%C(yellow)%an%C(reset) %C(dim)%ad%C(reset) %C(bold)%s%C(reset) %C(dim)%h%C(reset)%n%+b";
        log = {
          date = "relative";
          decorate = "auto";
        };
        blame.coloring = "highlightRecent";
        advice = {
          statusHints = false;
          addEmptyPathspec = false;
        };
        init.defaultBranch = "main";
        pull.rebase = true;
        rebase = {
          autoSquash = true;
          autoStash = true;
          updateRefs = true;
        };
        fetch = {
          prune = true;
          writeCommitGraph = true;
          recurseSubmodules = false;
          parallel = 0;
          negotiationAlgorithm = "skipping";
        };
        column.worktree = "auto";
        worktree.guessRemote = true;
        color = {
          ui = "auto";
          branch = "auto";
          diff = "auto";
          status = "auto";
          interactive = "auto";
        };
        core = {
          commitGraph = true;
          editor = "nvim";
          excludesfile = "~/.gitignore_global";
          multiPackIndex = true;
          pager = "delta";
          preloadindex = true;
          untrackedcache = true;
        };
        interactive.diffFilter = "delta --color-only";
        diff = {
          algorithm = "histogram";
          statNameWidth = 500;
          statGraphWidth = 500;
          external = "difft";
        };
        difftool.prompt = false;
        delta = {
          navigate = true;
          features = "interactive";
          side-by-side = true;
          pager = "less -RF --mouse";
        };
        merge = {
          conflictstyle = "diff3";
          renormalize = true;
          stat = false;
          ff = "only";
        };
        "merge \"ast-merge\"" = {
          name = "AST-aware merge driver";
          driver = "ast-merge merge %O %A %B --git";
        };
        rerere = {
          enabled = true;
          autoupdate = true;
        };
        index = {
          threads = 0;
          version = 4;
        };
        checkout.workers = 0;
        status = {
          submodulesummary = 0;
          aheadBehind = true;
        };
        submodule = {
          # Submodules are absent by default (see git-wt-submodules); populate a
          # worktree's submodules on demand. Recursing would make push/checkout
          # descend into absent submodule worktrees -> "not a git repository: '.git'".
          recurse = false;
          fetchJobs = 16;
        };
        push = {
          default = "simple";
          autoSetupRemote = true;
          recurseSubmodules = "no";
          followTags = true;
          negotiate = false;
        };
        "filter \"lfs\"" = {
          clean = "git-lfs clean -- %f";
          smudge = "git-lfs smudge -- %f";
          process = "git-lfs filter-process";
          required = true;
        };
        protocol.version = 2;
        http = {
          postBuffer = 524288000;
          maxRequestBuffer = "100M";
          lowSpeedLimit = 0;
          lowSpeedTime = 999999;
        };
        transfer.fsckObjects = false;
        receive.fsckObjects = false;
        uploadpack = {
          allowFilter = true;
          allowAnySHA1InWant = true;
        };
        feature.manyFiles = true;
        maintenance =
          {
            auto = true;
            strategy = "incremental";
          }
          # Registered maintenance repo lives on the workstation only.
          // lib.optionalAttrs pkgs.stdenv.isDarwin {
            repo = cfg.paths.ixCheckout;
          };
        sparse.expectFilesOutsideOfPatterns = true;
        "branchless.restack" = {
          preserveTimestamps = false;
          warnAbandoned = true;
        };
        "branchless.navigation".autoSwitchBranches = true;
        pager.log = true;
      }
      // gitCredentialHelpers
    );

  programs.ssh = let
    computeHosts =
      lib.mapAttrs (
        name: attrs:
          assert lib.assertMsg (
            attrs ? IdentityFile && attrs.IdentityFile != null && attrs.IdentityFile != ""
          ) "programs.ssh host '${name}' must set IdentityFile (no implicit ~/.ssh/id_rsa fallback)"; attrs
      )
      cfg.ssh.matchBlocks;
    pinnedKnownHosts = lib.optionalString (cfg.ssh.knownHosts != null) (toString (pkgs.writeText "ssh_known_hosts" cfg.ssh.knownHosts));
  in {
    enable = true;
    # All defaults live in the `*` settings block below; disable HM's
    # implicit "Host *" so we don't get a duplicate trailing block.
    enableDefaultConfig = false;
    settings =
      computeHosts
      // {
        "Host github.com gitlab.com bitbucket.org" = {
          ControlMaster = "auto";
          ControlPath = "~/.ssh/controlmasters/%r@%h-%p";
          ControlPersist = "600";
          ServerAliveInterval = 60;
          ServerAliveCountMax = 10;
        };
        "*" = {
          ForwardAgent = false;
          Compression = false;
          SetEnv = {
            TERM = "xterm-256color";
            COLORTERM = "truecolor";
          };
          # Forward LINEAR_API_KEY from the local shell env (resolved from
          # 1Password via nushell/secrets.template.nu) instead of baking the
          # secret into this tracked config. The remote sshd must `AcceptEnv` it,
          # and it only forwards when the launching shell has the var set.
          # NPM_TOKEN was removed: the old inline value leaked and is already
          # dead; re-add it the same way once a fresh token is in the pipeline.
          SendEnv = "LINEAR_API_KEY";
          AddKeysToAgent = "no";
          HashKnownHosts = false;
          # Writable user file first (ssh appends runtime-accepted hosts here),
          # immutable nix-pinned fleet keys second (verification only).
          UserKnownHostsFile = lib.concatStringsSep " " (["~/.ssh/known_hosts"] ++ lib.optional (pinnedKnownHosts != "") pinnedKnownHosts);
          # No IdentityAgent: auth uses on-disk keys only. The 1Password agent
          # was removed here because a locked app blocked every ssh/sign for
          # agents (auto-memory vc1-ssh-bypass-1password). It held two keys,
          # both now on disk: id_ed25519 (the signing key, pinned per host
          # above) and id_ed25519_legacy ("main ssh key", exported via
          # `op read`), offered as a fallback for hosts that only ever
          # authorized the latter. Both are listed here: any IdentityFile in
          # `*` disables ssh's built-in default list, so id_ed25519 must be
          # restated or unlisted hosts would offer only the legacy key.
          IdentityFile = [
            "~/.ssh/id_ed25519"
            "~/.ssh/id_ed25519_legacy"
          ];
        };
      };
  };

  # NOTE: allowUnfree is NOT set here. The standalone Mac config gets it from the
  # externally-provided pkgs (flake.nix `mkHomeConfig`), and the VM gets it from
  # the NixOS system pkgs (hosts/vm). Setting `nixpkgs.config` in a home module
  # is a no-op (and a deprecation warning) under both `useGlobalPkgs` and an
  # externally-provided pkgs, so it lives at the system / pkgs layer instead.
}
