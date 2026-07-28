{
  lib,
  skills,
}:
# Build a Claude Code plugin directory, loaded into the agent via the wrapper's
# `--plugin-dir` (claude-code's `pluginDirs`). A plugin bundles skills (which the
# agent invokes as `/<plugin-name>:<skill>`) and, optionally, hooks. It
# deliberately does NOT bundle agents: a plugin namespaces a subagent's
# `subagent_type` (`<plugin>:<agent>`), which breaks any bare agent reference
# (e.g. a `code-reviewer` invoked by name from a hook or another skill), so
# agents are delivered through `.claude/agents` (see `agents.mkAgentsDir`).
let
  mkPlugin = {
    pkgs,
    name,
    version ? "0.1.0",
    description ? "Claude Code plugin: ${name}",
    names ? skills.allSkills,
    extraSkills ? {},
    hooks ? null,
  }: let
    collisions = lib.intersectLists names (builtins.attrNames extraSkills);
    skillsDir = skills.mkSkillsDir {inherit pkgs names extraSkills;};
    manifest = (pkgs.formats.json {}).generate "claude-plugin-${name}-manifest.json" {
      inherit name version description;
    };
    hooksFile = (pkgs.formats.json {}).generate "claude-plugin-${name}-hooks.json" {
      inherit hooks;
    };
  in
    assert lib.assertMsg (collisions == [])
    "claudePlugin.mkPlugin: extraSkills name(s) collide with index skills: ${lib.concatStringsSep ", " collisions}";
      pkgs.runCommand "claude-plugin-${name}" {
        # Manifest identity for consumers that address the plugin by name and
        # version (e.g. Codex's plugins/cache/<marketplace>/<plugin>/<version>
        # install path).
        passthru = {
          pluginName = name;
          inherit version;
        };
      } ''
        mkdir -p "$out/.claude-plugin"
        cp ${manifest} "$out/.claude-plugin/plugin.json"
        cp -RL ${skillsDir} "$out/skills"
        ${lib.optionalString (hooks != null) ''
          mkdir -p "$out/hooks"
          cp ${hooksFile} "$out/hooks/hooks.json"
        ''}
      '';
  # A marketplace wraps plugins one directory level up so runtimes that only
  # install from marketplaces (Codex: `[marketplaces.<name>]` config with
  # `source_type = "local"`) can consume the same plugin dirs Claude Code
  # loads directly via `--plugin-dir`. Codex resolves
  # `.claude-plugin/marketplace.json` and `.claude-plugin/plugin.json` as
  # compatible manifest locations, so one store path serves both ecosystems.
  mkMarketplace = {
    pkgs,
    name,
    owner ? "indexable-inc",
    # Attrset from plugin name to a plugin directory (an `mkPlugin` output).
    plugins,
  }: let
    manifest = (pkgs.formats.json {}).generate "claude-marketplace-${name}-manifest.json" {
      inherit name;
      owner.name = owner;
      plugins =
        lib.mapAttrsToList (pluginName: _: {
          name = pluginName;
          source = "./plugins/${pluginName}";
        })
        plugins;
    };
  in
    pkgs.runCommand "claude-marketplace-${name}" {} ''
      mkdir -p "$out/.claude-plugin" "$out/plugins"
      cp ${manifest} "$out/.claude-plugin/marketplace.json"
      ${lib.concatStringsSep "\n" (lib.mapAttrsToList (
          pluginName: pluginDir: ''cp -RL ${pluginDir} "$out/plugins/${pluginName}"''
        )
        plugins)}
    '';
in {
  /**
  Build a Claude Code plugin directory for `--plugin-dir`.

  Arguments:
  - `pkgs`: the package set used to build the directory.
  - `name`: the plugin name; also the `/`-invocation namespace for its skills
    (`/<name>:<skill>`). Written to `.claude-plugin/plugin.json`.
  - `version`, `description`: plugin manifest metadata (sensible defaults).
  - `names`: which discovered skills to include. Defaults to every index skill.
  - `extraSkills`: attrset from name to path for skills outside this repo,
    merged in (see `skills.mkSkillsDir`). Must not collide with `names`.
  - `hooks`: an optional Claude hooks object (`{ <EventName> = [ ... ]; }`,
    the same shape as a settings.json `hooks` value). When set, it is written
    to `hooks/hooks.json` so the plugin carries the hooks too. `null` (default)
    ships no hooks.

  Returns a symlink-free directory holding `.claude-plugin/plugin.json`,
  `skills/<name>/SKILL.md`, and optionally `hooks/hooks.json`. Deliver it to
  the agent by baking `--plugin-dir=<this>` (claude-code's `pluginDirs`).
  */
  inherit mkPlugin;

  /**
  Wrap `mkPlugin` outputs into a Claude-format marketplace directory
  (`.claude-plugin/marketplace.json` + `plugins/<name>/`), the layout Codex
  installs from via a `[marketplaces.<name>]` config entry with
  `source_type = "local"` and `source = <this dir>`.

  Arguments:
  - `pkgs`: the package set used to build the directory.
  - `name`: the marketplace name (plugin ids become `<plugin>@<name>`).
  - `owner`: marketplace manifest owner metadata.
  - `plugins`: attrset from plugin name to plugin directory.
  */
  inherit mkMarketplace;
}
