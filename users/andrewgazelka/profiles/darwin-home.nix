# macOS-specific personal Home Manager profile.
{
  configRoot,
  ghosttyModule,
  indexPackages,
  ix,
  optionsModule,
  raycastModule,
  symphonyModule,
}: {
  config,
  pkgs,
  lib,
  ...
}: let
  cfg = config.users.andrewgazelka;
  indexPkgs = indexPackages pkgs.stdenv.hostPlatform.system;
  repoRoot = configRoot;
  repoFile = rel: repoRoot + "/${rel}";
  structured = import (configRoot + "/settings/structured.nix");
  textStructured = import (configRoot + "/settings/text.nix");
  tomlFormat = pkgs.formats.toml {};
  cursorSettings = builtins.fromJSON (
    builtins.replaceStrings
    ["/Users/andrewgazelka"]
    [config.home.homeDirectory]
    (builtins.toJSON structured.cursor-settings.value)
  );

  grayscaleSwift = configRoot + "/scripts/grayscale.swift";

  # Reusable "do not overlap" wrapper for launchd agents. Grabs a NON-BLOCKING
  # exclusive lock keyed by the agent label; if the previous run is still going
  # the new fire exits 0 silently (skipped) instead of overlapping, and the lock
  # releases on exit including crash/kill. macOS has no stock flock(1), so the
  # lock is taken via /usr/bin/perl + flock(2). See scripts/with-lock.sh.
  withLock = ix.writeBashApplication pkgs {
    name = "with-lock";
    runtimeInputs = [pkgs.coreutils];
    text = builtins.readFile (configRoot + "/scripts/with-lock.sh");
  };

  # DRY helper: wrap a launchd agent's ProgramArguments so the run is serialized
  # under a per-label lock. `label` namespaces the lock (each agent only blocks
  # itself); `args` is the original ProgramArguments list. Use as:
  #   ProgramArguments = lockArgs "main-sync" [ "${mainSync}/bin/main-sync" ];
  lockArgs = label: args:
    [
      "${withLock}/bin/with-lock"
      label
      "--"
    ]
    ++ args;

  # pr-watch + ci-triage + announce-lib live in the shared users/andrewgazelka
  # module (index `homeModules.andrewgazelka`). pr-watch is disabled below
  # because that upstream watcher still queues xp-orb overlay events.

  # Fast-forwards the shared main checkouts of ix + index to origin/main (polled
  # by the main-sync launchd agent) so worktrees always branch from fresh main.
  # Only advances main when it is checked out, clean, and a fast-forward; never
  # touches WIP. runtimeInputs bakes git onto PATH for launchd's minimal env.
  mainSync = ix.writeBashApplication pkgs {
    name = "main-sync";
    # gh is the git credential helper for the https origins; without it on
    # PATH the authenticated fetch fails under launchd's minimal environment.
    runtimeInputs = [
      pkgs.git
      pkgs.gh
    ];
    text = builtins.readFile (configRoot + "/scripts/main-sync.sh");
  };

  # Single owner of the login-Keychain secrets that headless launchd agents read
  # at runtime (ix-downtime's Better Stack token, pr-watch ci-triage's Linear
  # key). Headless agents can't unlock op/rbw, so secrets are staged in the
  # Keychain; this command maps each Keychain service to its canonical op/rbw
  # source in one place and seeds them idempotently. Run once after `op signin` /
  # `rbw unlock`. op/rbw come from the interactive PATH (op is Homebrew), so it
  # is not wired into runtimeInputs. See scripts/seed-launchd-secrets.sh.
  seedLaunchdSecrets = ix.writeBashApplication pkgs {
    name = "seed-launchd-secrets";
    text = ''exec ${lib.escapeShellArg "${cfg.paths.privateConfigDirectory}/scripts/seed-launchd-secrets.sh"} "$@"'';
  };

  # Watchdog for the recurring Finder /nix/store SynchronizeChildren CPU spin
  # (https://github.com/andrewgazelka/nix/issues/66). All detection and
  # remediation logic (two-sample CPU, `sample` signature check, pref delete +
  # SIGKILL, notification + log) lives in scripts/finder-spin-watchdog.sh; the
  # launchd agent below fires it every 5 minutes.
  finderSpinWatchdog = ix.writeBashApplication pkgs {
    name = "finder-spin-watchdog";
    runtimeInputs = [pkgs.coreutils];
    text = builtins.readFile (configRoot + "/scripts/finder-spin-watchdog.sh");
  };

  # Browser-tab blocklist (flake.nix `cfg.browserBlockedHosts`): every 1s, scan
  # Safari/Chrome/Dia tabs over Apple Events and redirect any tab on a blocked
  # site (apex + www) to localhost. The browser-interaction complement to the
  # /etc/hosts sinkhole: the network stays open for everything else that talks
  # to the domain (OAuth, APIs), only the website itself is unusable. Needs a
  # one-time Automation (TCC) approval per browser on first fire. The system
  # osascript is used because JXA ships with macOS; see scripts/tab-blocklist.js
  # for the Dia quirks the script works around.
  tabBlocklist = ix.writeBashApplication pkgs {
    name = "tab-blocklist";
    text = ''
      while :; do
        /usr/bin/osascript -l JavaScript ${configRoot + "/scripts/tab-blocklist.js"} \
          ${lib.escapeShellArgs cfg.browserBlockedHosts} || true
        sleep 1
      done
    '';
  };
in {
  # General Raycast Focus module lives in index (homeModules.raycast); this is
  # the personal config consuming it. Mechanism in index, values here.
  imports = [
    optionsModule
    raycastModule
    # Symphony BEAM runtime (mechanism in index, values in the
    # services.symphony block below): scheduled agent workflows with Slack
    # digests. Renders a launchd agent here via portable-services.
    symphonyModule
    # Ghostty config, generated from Nix (home/ghostty.nix). Replaces the former
    # out-of-store symlink to ghostty/config.
    ghosttyModule
  ];

  home.sessionPath = ["$HOME/.lmstudio/bin"];

  # Symphony runtime: the BEAM is the scheduler (cron triggers tick inside
  # it), so the agent stays resident (restart=always in the home module).
  # packDir points at the mutable index checkout: editing a .sym or prompt
  # there applies live, no restart. A lid-closed 9am fire is deferred to at
  # most one catch-up run on wake (CronState watermark); an always-on linux
  # host can enable this same module for hard scheduling guarantees.
  # Secrets (SLACK_BOT_OAUTH_TOKEN) live in the environmentFile, never the
  # store; seed it from the team vault (see README in that directory).
  services.symphony = {
    enable = true;
    primaryRepo = cfg.paths.indexCheckout;
    packDir = cfg.paths.symphonyPack;
    environmentFile = "${config.home.homeDirectory}/.config/symphony/env";
    extraPath = [
      # Plain upstream codex, NOT the index wrapper: the wrapper bakes the
      # ix-mcp/exa MCP servers as argv -c flags that --ignore-user-config
      # cannot remove, and their bootstrap wedges unattended launchd runs
      # (verified live 2026-07-06: 0% CPU codex, MCP init never completed).
      pkgs.codex
      # Plain upstream claude-code for the same reason: the ~/.local/bin
      # wrapper's baked ix-mcp bootstrap wedges and chats on stdout in
      # unattended launchd runs (04:30Z truncated-reply tick). The index
      # build, not nixpkgs' (whose fetcher needs __noChroot, blocked by
      # the darwin sandbox).
      indexPkgs.claude-code
      pkgs.jq
      pkgs.gh
      pkgs.git
    ];
    extraEnvironment = {
      SYMPHONY_SLACK_NOTIFY_CHANNEL = "C0A4TD9G7HR"; # #general
      SYMPHONY_SLACK_NOTIFY_CRON_WORKFLOWS = "insights,triage";
      # Standing local room-server (tmux session `room-server`, gc-rooted
      # ix#room-server at ~/.local/share/room-server/app) for :local /
      # {:room, url} placements; agent threads are watchable at this URL.
      SYMPHONY_ROOM_SERVER_URL = "http://127.0.0.1:3010";
      # Compiled overseer report app (template.html + bundle.js); the
      # overseer workflow tick splices its data.json into the template.
      # TODO(darwin-cache): flip to the declarative reference once the
      # e9ef6062 cache-ready pin's native darwin codex substitutes (the
      # 21:56Z switch attempt built codex locally and failed); until then
      # the env-file bridge carries the gc-rooted store path.
      # OVERSEER_APP = "${indexPkgs.overseer-report}";
    };
  };

  # Raycast Focus session defaults, written to the com.raycast.macos defaults
  # domain at switch time. The blocklist itself stays UI-managed: Raycast
  # enforces blocking from its own encrypted DB, not this plist, so a
  # plist-declared blocklist is cosmetic and does not actually block. Only the
  # clean knobs (title/mode/duration) are declared here.
  programs.raycast.focus = {
    enable = true;
    title = "Nix";
    filterMode = "block";
    duration.seconds = 900;
    duration.title = "15 minutes";
  };

  # macOS-only packages
  home.packages = [
    # pkgs.notify
    cfg.packages.lifelog # `lifelog top` / `sqlite3 "$(lifelog db-path)"`; the recorder runs as the services.portable.lifelog launchd agent
    pkgs.duti
    pkgs.iproute2mac
    pkgs.libimobiledevice # `idevice_id -l` / `ideviceinfo` / `idevicepair`: talk to a USB-attached iPhone over Apple's usbmuxd

    pkgs.pinentry_mac # GUI master-password prompt used by `rbw login`/`rbw unlock` (rbw itself is in common.nix)
    seedLaunchdSecrets # `seed-launchd-secrets`: stage launchd-agent Keychain secrets from op/rbw
  ];

  home.sessionVariables = {
    # Fish never sources nix-darwin's set-environment (only zsh/bash do via
    # /etc/zshenv and /etc/bashrc), so interactive fish historically inherited
    # this from however the terminal happened to be launched. Set it here so
    # hm-session-vars provides it to every shell: nixpkgs curl/openssl (and
    # therefore git) need it to find the system CA bundle.
    NIX_SSL_CERT_FILE = "/etc/ssl/certs/ca-certificates.crt";
  };

  # ============================================
  # Lint Claude skills/commands/agents before every switch
  # ============================================
  # A workstation guard, so it lives in this macOS-only module rather than
  # the workstation profile: hydra is where Claude actually runs, and the
  # headless VM imports only the portable index-free profiles. Runs before
  # writeBoundary so a failure aborts cleanly without touching the profile.
  # skillsaw (https://skillsaw.org) validates frontmatter,
  # structure, context budget, weak language, and cross-file consistency; uvx
  # fetches it from PyPI on first use and caches it, so later runs are offline.
  # It lints the exact tracked source that the generation will deploy.
  home.activation.lintClaudeFiles = config.lib.dag.entryBefore ["writeBoundary"] ''
    if [ -d ${configRoot + "/claude/global"} ]; then
      echo "Linting Claude skills, commands, and agents…"
      ${pkgs.uv}/bin/uvx --quiet skillsaw lint \
        --type dot-claude \
        ${configRoot + "/claude/global"} || {
          echo "✗ skillsaw lint failed — aborting home-manager switch" >&2
          echo "  Fix users/andrewgazelka/config/claude/global in index." >&2
          exit 1
        }
    fi
  '';

  assertions = [
    {
      assertion = cfg.packages.lifelog != null;
      message = "users.andrewgazelka.packages.lifelog must be set for the Darwin home profile.";
    }
    {
      assertion = cfg.rbw.email != null && cfg.rbw.baseUrl != null;
      message = "users.andrewgazelka.rbw email and baseUrl must be set for the Darwin profile.";
    }
  ];

  # ============================================
  # Disable macOS service keyboard shortcuts
  # ============================================
  home.activation.disableServiceShortcuts = config.lib.dag.entryAfter ["writeBoundary"] ''
    /usr/bin/defaults write pbs NSServicesStatus \
      -dict-add 'com.apple.Safari - Search With Google - searchWithGoogle' \
      '{ "enabled_context_menu" = 1; "enabled_services_menu" = 1; "key_equivalent" = ""; }'
  '';

  # ============================================
  # Default file associations
  # ============================================
  home.activation.setDefaultApps = let
    associations = {
      # Open SVGs in the Dia browser (renders them) instead of an editor.
      "company.thebrowser.dia" = [
        "svg"
      ];
      # Ghostty as the default terminal: run terminal-script files in it.
      # Only the "run in terminal" extensions go here (.command/.tool); .sh
      # and friends stay mapped to IntelliJ below for editing. The core
      # default-terminal association (the public.unix-executable UTI) is set
      # separately below, since that's a UTI rather than a file extension.
      "com.mitchellh.ghostty" = [
        "command"
        "tool"
      ];
      "com.apple.QuickTimePlayerX" = [
        "mp3"
        "m4a"
        "wav"
        "aac"
        "aiff"
        "flac"
        "ogg"
        "wma"
      ];
      "com.jetbrains.intellij" = [
        "rs"
        "toml"
        "nix"
        "json"
        "yaml"
        "yml"
        "xml"
        "css"
        "js"
        "ts"
        "tsx"
        "jsx"
        "md"
        "txt"
        "sh"
        "bash"
        "zsh"
        "fish"
        "py"
        "rb"
        "go"
        "java"
        "kt"
        "swift"
        "c"
        "cpp"
        "h"
        "hpp"
        "sql"
        "graphql"
        "proto"
        "kdl"
        "conf"
        "cfg"
        "ini"
        "env"
        "lock"
        "log"
        "csv"
        "tsv"
      ];
    };
    dutiCommands = builtins.concatStringsSep "\n" (
      builtins.concatLists (
        lib.mapAttrsToList (
          bundleId: exts:
            map (ext: ''${pkgs.duti}/bin/duti -s "${bundleId}" ".${ext}" all 2>/dev/null || true'') exts
        )
        associations
      )
      ++ [
        # Make Ghostty the default terminal: assign it the `shell` role for
        # the public.unix-executable UTI. This is exactly what iTerm2's "Make
        # iTerm2 Default Term" menu item does, and the modern equivalent of
        # NSWorkspace.setDefaultApplication(toOpen: .unixExecutable) /
        # LSSetDefaultRoleHandlerForContentType(..., kLSRolesShell). macOS has
        # no other "default terminal" concept. duti takes the UTI directly
        # (no leading dot); `shell` = "application can execute the item".
        ''${pkgs.duti}/bin/duti -s "com.mitchellh.ghostty" public.unix-executable shell 2>/dev/null || true''
      ]
    );
  in
    config.lib.dag.entryAfter ["writeBoundary"] dutiCommands;

  # ============================================
  # macOS-specific paths (Library/Application Support)
  # ============================================

  # Ghostty main config: generated from Nix in home/ghostty.nix (imported above).

  # Claude Desktop rewrites this file and carries a runtime Authorization
  # header. Reconcile only public keys; mutable-json preserves the unmanaged
  # credential without ever copying it into the Nix store.
  # BlenderMCP addon auto-load: the in-Blender half of the MCP bridge, from the
  # SAME pinned rev as the `blender-mcp` server binary (index packages/blender-mcp
  # passthru.addon), so the :9876 socket protocol cannot drift between them.
  # NOTE: 9876 is also the Lab addon's compiled-in default; the Lab startup
  # script below re-ports it to the registry's 9877 before starting it.
  home.file."Library/Application Support/Blender/5.1/scripts/startup/blender_mcp.py".text = ''
    import importlib.util
    import sys

    import bpy

    if not hasattr(bpy.types, "blendermcp_server"):
        spec = importlib.util.spec_from_file_location(
            "blender_mcp_addon",
            "${indexPkgs.blender-mcp.passthru.addon}",
        )
        module = importlib.util.module_from_spec(spec)
        sys.modules["blender_mcp_addon"] = module
        spec.loader.exec_module(module)
        module.register()
  '';

  # Official Blender Lab MCP addon, the in-Blender half of the `blender-lab`
  # bridge (index packages/blender-lab-mcp passthru.addon, same pinned rev as
  # the server). Linked under a name DIFFERENT from upstream's
  # `blender_mcp_addon` because the community addon's loader above already owns
  # that sys.modules key. The registry entry is the source of truth for the
  # addon's port below.
  home.file."Library/Application Support/Blender/5.1/scripts/addons/blender_lab_mcp_addon".source =
    indexPkgs.blender-lab-mcp.passthru.addon;

  home.file."Library/Application Support/Blender/5.1/scripts/startup/blender_lab_mcp.py".text = let
    # `blenderLabMcp` value is unused for the port lookup; only `env` is read.
    labPort =
      (ix.mcp.optionalServers {blenderLabMcp = "unused";})
        .blender-lab.env.BLENDER_MCP_PORT;
  in
    # Deferred to a timer: startup-script module bodies run under Blender's
    # restricted context, and the whole setup must be fail-loud. The explicit
    # server_start below wins the race against the addon's own autostart
    # timer (scheduled at register with delay 1.0s, and its _autostart_timer
    # no-ops behind an is_running() guard once we hold the socket); turning
    # use_autostart OFF then keeps the terminal state deterministic, so later
    # launches with persisted prefs never schedule autostart at all. That
    # matters because the addon's compiled-in DEFAULT_PORT (9876) collides
    # with the community addon; assumes autostart_delay stays at its 1.0s
    # default (nothing here lowers it).
    ''
      import bpy


      def _blender_lab_mcp_setup():
          import addon_utils

          try:
              # The addon gates even its LOOPBACK socket on Blender's "Allow
              # Online Access" preference (addon __init__.py:
              # startup_online_ok_or_error); real network policy stays with
              # the host firewall.
              bpy.context.preferences.system.use_online_access = True

              # default_set=True: only the default set materializes an entry
              # in preferences.addons, and the preferences below live on that
              # entry (verified live: default_set=False leaves the key absent).
              module = addon_utils.enable(
                  "blender_lab_mcp_addon", default_set=True, persistent=True
              )
              if module is None:
                  print("blender_lab_mcp: addon enable FAILED (import error?)")
                  return None
              prefs = bpy.context.preferences.addons[
                  "blender_lab_mcp_addon"
              ].preferences
              prefs.use_autostart = False
              prefs.port = ${labPort}
              # A bind failure does NOT raise: the operator reports the error
              # into the UI and returns CANCELLED, so check the result set.
              result = bpy.ops.blmcp.server_start()
              if "FINISHED" in result:
                  print("blender_lab_mcp: server started on port", prefs.port)
              else:
                  print("blender_lab_mcp: server_start FAILED:", result)
          except Exception as exc:
              print("blender_lab_mcp: setup FAILED:", exc)
          return None


      # 0.5s: after Blender finishes initializing (full context available),
      # before any human interaction matters.
      bpy.app.timers.register(_blender_lab_mcp_setup, first_interval=0.5)
    '';

  # Cursor and VS Code rewrite settings during UI changes. Keep the files
  # writable while reconciling the Nix-owned keys on activation.
  home.mutableJsonFiles = {
    claude-desktop = {
      target = "Library/Application Support/Claude/claude_desktop_config.json";
      value = {
        globalShortcut = "";
        mcpServers.ix = {
          type = "http";
          url = "https://mcp.ix.dev/mcp";
          headers = {};
        };
        preferences = {
          quickEntryShortcut = "off";
          quickEntryDictationShortcut = "off";
          localAgentModeTrustedFolders = [cfg.paths.ixCheckout];
          coworkScheduledTasksEnabled = true;
          ccdScheduledTasksEnabled = true;
          sidebarMode = "code";
          bypassPermissionsModeEnabled = true;
          coworkWebSearchEnabled = true;
          coworkOnboardingResumeStep = null;
          keepAwakeEnabled = true;
          dispatchCodeTasksPermissionMode = "bypassPermissions";
        };
      };
    };
    cursor-settings = {
      target = "Library/Application Support/Cursor/User/settings.json";
      value = cursorSettings;
    };
    vscode-settings = {
      target = "Library/Application Support/Code/User/settings.json";
      value = cursorSettings;
    };
    rbw = {
      target = "Library/Application Support/rbw/config.json";
      value = {
        email = cfg.rbw.email;
        base_url = cfg.rbw.baseUrl;
        pinentry = "${cfg.paths.privateConfigDirectory}/rbw/op-pinentry.sh";
        lock_timeout = 3600;
        sync_interval = 3600;
      };
    };
  };

  # Keybindings use JSONC and cannot use the JSON reconciler.
  home.file."Library/Application Support/Cursor/User/keybindings.json".text =
    textStructured.cursor-keybindings-json;
  home.file."Library/Application Support/Code/User/keybindings.json".text =
    textStructured.cursor-keybindings-json;

  # Bacon
  home.file."Library/Application Support/org.dystroy.bacon/prefs.toml".source =
    tomlFormat.generate "andrewgazelka-bacon-prefs.toml" structured.bacon-prefs.value;

  # Zen browser
  home.file."Library/Application Support/zen/Profiles/nu2gused.Default (release)/chrome".source =
    repoFile "zen/chrome";

  # Beeper
  home.file."Library/Application Support/BeeperTexts/custom.css".source =
    repoFile "beeper/custom.css";

  # Nushell config (macOS uses Library/Application Support, not XDG)
  home.file."Library/Application Support/nushell".source =
    repoFile "nushell";

  # rbw (Vaultwarden CLI). On macOS rbw reads its config from
  # Library/Application Support, not XDG, so the upstream programs.rbw module
  # (which writes ~/.config/rbw) does not apply. Symlinked (not store-baked) so
  # the JSON stays editable without a re-switch; `pinentry` is a bare name
  # resolved from PATH (pinentry_mac is in this file's packages) so it survives
  # nixpkgs bumps. Secrets resolve via `bw://ix-infra/<item>/<field>` after
  # `rbw login` prompts for the master password through pinentry-mac.
  # ============================================
  # launchd services (macOS service manager)
  # ============================================

  # Browser-tab blocklist agent: long-lived poller (KeepAlive), relaunched by
  # launchd if it dies. Only fires Apple Events when a browser is running, so
  # idle cost is negligible. The blocked-domain list comes from flake.nix.
  launchd.agents.tab-blocklist = {
    enable = cfg.browserBlockedHosts != [];
    config = {
      ProgramArguments = ["${tabBlocklist}/bin/tab-blocklist"];
      KeepAlive = true;
      RunAtLoad = true;
      StandardOutPath = "${config.home.homeDirectory}/Library/Logs/tab-blocklist.log";
      StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/tab-blocklist.log";
    };
  };

  # Machine-wide nix build dashboard: `nix-web-monitor serve` on :7532 at login
  # (indexable-inc/index#2182, PR#2185). No wrapped command: the machine-builds
  # panel polls `nix store builds --json`, which needs the patched nix
  # (2.34.7+ix, build-status-dir) -- hence the explicit PATH pointing at the
  # system profile; launchd's default PATH has no nix at all. KeepAlive
  # restarts it on crash; the lock stops a respawn from fighting a live
  # instance for the port.
  launchd.agents.nix-web-monitor = {
    enable = true;
    config = {
      ProgramArguments = lockArgs "nix-web-monitor" [
        "${indexPkgs.nix-web-monitor}/bin/nix-web-monitor"
        "serve"
      ];
      EnvironmentVariables.PATH = "/run/current-system/sw/bin:/usr/bin:/bin:/usr/sbin:/sbin";
      KeepAlive = true;
      RunAtLoad = true;
      StandardOutPath = "${config.home.homeDirectory}/Library/Logs/nix-web-monitor.log";
      StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/nix-web-monitor.log";
    };
  };

  # Atuin daemon for automatic syncing
  launchd.agents.atuin-daemon = {
    enable = true;
    config = {
      ProgramArguments = lockArgs "atuin-daemon" [
        "${pkgs.atuin}/bin/atuin"
        "daemon"
      ];
      KeepAlive = true;
      RunAtLoad = true;
      StandardOutPath = "${config.home.homeDirectory}/Library/Logs/atuin-daemon.log";
      StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/atuin-daemon.log";
    };
  };

  # Grayscale display: on at 17:00, off at 07:00.
  # Uses the UniversalAccess private framework (UAGrayscaleSetEnabled) which
  # applies instantly without a logout, unlike `defaults write`.
  launchd.agents.grayscale-on = {
    enable = false;
    config = {
      ProgramArguments = lockArgs "grayscale-on" [
        "/usr/bin/swift"
        "${grayscaleSwift}"
        "on"
      ];
      StartCalendarInterval = [
        {
          Hour = 17;
          Minute = 0;
        }
      ];
      StandardOutPath = "${config.home.homeDirectory}/Library/Logs/grayscale.log";
      StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/grayscale.log";
    };
  };

  # The merged-PR + CI-failure watcher (`pr-watch`, every 30s) with its detached
  # `ci-triage` stage-2 and the `/optimize` history scan are declared by the
  # hoisted users/andrewgazelka module (index `homeModules.andrewgazelka`,
  # imported by profiles/workstation.nix). It renders native launchd agents (a
  # StartInterval=30 watcher). Host glue stays here: the watcher reads its secret
  # from the login Keychain (`pr-watch-linear` for ci-triage's ticket filing),
  # seeded by `seed-launchd-secrets`. logDir keeps the logs in the macOS
  # `~/Library/Logs` convention. pr-watch defaults to watching indexable-inc/ix +
  # index; override `prWatch.repos` to change that.
  #
  # Minecraft overlays are off. pr-watch is disabled too because the pinned
  # upstream script always queues `xp-orb-overlay push` events.
  users.andrewgazelka = {
    logDir = "${config.home.homeDirectory}/Library/Logs";
    downtime.enable = false;
    bossbarOverlay.enable = false;
    mergeOrbOverlay.enable = false;
    prWatch.enable = false;

    # Continuous activity recorder (github:andrewgazelka/lifelog): frontmost
    # app + idle + lock into ~/Library/Application Support/lifelog/lifelog.db,
    # Screen Time ingest from knowledgeC.db, and a localhost phone-event API.
    # Query with: sqlite3 "$(lifelog db-path)" or `lifelog top`.
    # The Screen Time ingest needs Full Disk Access for the lifelog binary
    # (System Settings > Privacy & Security); the sampler records regardless.
    lifelog = {
      enable = true;
      package = cfg.packages.lifelog;
    };
  };

  users.andrewgazelka.ciBars.enable = false;

  # Main sync: every minute, fast-forward ix + index main checkouts to
  # origin/main. Conservative (clean + ff-only), so it never disturbs WIP.
  launchd.agents.main-sync = {
    enable = true;
    config = {
      ProgramArguments = lockArgs "main-sync" ["${mainSync}/bin/main-sync"];
      StartInterval = 60;
      RunAtLoad = true;
      StandardOutPath = "${config.home.homeDirectory}/Library/Logs/main-sync.log";
      StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/main-sync.log";
    };
  };

  # Finder /nix/store spin watchdog (issue #66): every 5 minutes, if Finder
  # burns > 50% real CPU (macOS `ps` lies for Finder; the script uses a
  # two-sample `top -l 2`) AND a `sample` shows DesktopServices
  # TNode::SynchronizeChildren, it deletes FXRecentFolders then SIGKILLs
  # Finder, posting a notification and logging the sample excerpt. Root cause
  # and live validation history:
  # claude/auto-memory/finder-nix-store-recent-folder-spin.md. The lock keeps
  # a slow `top` + `sample` run from overlapping the next 5-minute fire.
  launchd.agents.finder-nix-spin-watchdog = {
    enable = true;
    config = {
      ProgramArguments = lockArgs "finder-nix-spin-watchdog" [
        "${finderSpinWatchdog}/bin/finder-spin-watchdog"
      ];
      StartInterval = 300;
      RunAtLoad = true;
      StandardOutPath = "${config.home.homeDirectory}/Library/Logs/finder-spin-watchdog.log";
      StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/finder-spin-watchdog.log";
    };
  };

  # Standing local room-server (agent-thread host + viewer). HTTP port
  # 3010 = the fleet's claim (ix lib/ports.nix platform.room.http), reused
  # locally so one registry owns the number; WebTransport stays on 4433. The binary is the gc-rooted out-link of
  # ix#room-server (config has no ix input; refresh with
  # `nix build ~/Projects/indexable-inc/ix#room-server --out-link
  # ~/.local/share/room-server/app`). Symphony reaches it via
  # SYMPHONY_ROOM_SERVER_URL above.
  launchd.agents.room-server = {
    enable = true;
    config = {
      ProgramArguments = lockArgs "room-server" [
        "${config.home.homeDirectory}/.local/share/room-server/app/bin/room-server"
      ];
      EnvironmentVariables.ROOM_PORT = "3010";
      # launchd starts agents at /; room-server opens a relative room.db.
      WorkingDirectory = "${config.home.homeDirectory}/.local/share/room-server/state";
      KeepAlive = true;
      RunAtLoad = true;
      StandardOutPath = "${config.home.homeDirectory}/Library/Logs/room-server.log";
      StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/room-server.log";
    };
  };

  launchd.agents.grayscale-off = {
    enable = false;
    config = {
      ProgramArguments = lockArgs "grayscale-off" [
        "/usr/bin/swift"
        "${grayscaleSwift}"
        "off"
      ];
      StartCalendarInterval = [
        {
          Hour = 7;
          Minute = 0;
        }
      ];
      StandardOutPath = "${config.home.homeDirectory}/Library/Logs/grayscale.log";
      StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/grayscale.log";
    };
  };
}
