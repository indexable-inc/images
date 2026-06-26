{ lib, skills }:
# Build a Codex plugin directory and local marketplace from the same skill source
# set that backs Claude Code plugins. Codex plugins use `.codex-plugin/plugin.json`
# and are discovered through a marketplace file under `.agents/plugins/`.
let
  marketplaceEntry =
    {
      name,
      path,
      category ? "Productivity",
      installation ? "AVAILABLE",
      authentication ? "ON_INSTALL",
      displayName ? name,
      shortDescription ? null,
    }:
    {
      inherit name category;
      source = {
        source = "local";
        inherit path;
      };
      policy = {
        inherit installation authentication;
      };
      interface = {
        inherit displayName;
      }
      // lib.optionalAttrs (shortDescription != null) {
        inherit shortDescription;
      };
    };

  mkPlugin =
    {
      pkgs,
      name,
      version ? "0.1.0",
      description ? "Codex plugin: ${name}",
      names ? skills.allSkills,
      extraSkills ? { },
      skillsDir ? skills.mkSkillsDir { inherit pkgs names extraSkills; },
      hooks ? null,
    }:
    let
      collisions = lib.intersectLists names (builtins.attrNames extraSkills);
      manifest =
        (pkgs.formats.json { }).generate "codex-plugin-${name}-manifest.json" (
          {
            inherit name version description;
            skills = "./skills/";
          }
          // lib.optionalAttrs (hooks != null) {
            hooks = "./hooks/hooks.json";
          }
        );
      hooksFile = (pkgs.formats.json { }).generate "codex-plugin-${name}-hooks.json" {
        inherit hooks;
      };
    in
    assert lib.assertMsg (collisions == [ ])
      "codexPlugin.mkPlugin: extraSkills name(s) collide with index skills: ${lib.concatStringsSep ", " collisions}";
    pkgs.runCommand "codex-plugin-${name}" { } ''
      mkdir -p "$out/.codex-plugin"
      cp ${manifest} "$out/.codex-plugin/plugin.json"
      cp -RL ${skillsDir} "$out/skills"
      ${lib.optionalString (hooks != null) ''
        mkdir -p "$out/hooks"
        cp ${hooksFile} "$out/hooks/hooks.json"
      ''}
      links=$(find "$out" -type l)
      if [ -n "$links" ]; then
        echo "codex-plugin: symlinks survived materialization:" >&2
        echo "$links" >&2
        exit 1
      fi
    '';

  mkMarketplace =
    {
      pkgs,
      name,
      plugins,
    }:
    let
      marketplace = (pkgs.formats.json { }).generate "codex-plugin-marketplace-${name}.json" {
        inherit name plugins;
      };
    in
    pkgs.runCommand "codex-plugin-marketplace-${name}" { } ''
      mkdir -p "$out"
      cp ${marketplace} "$out/marketplace.json"
    '';
in
{
  inherit mkPlugin mkMarketplace marketplaceEntry;
}
