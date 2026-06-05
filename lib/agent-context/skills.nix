{ lib, paths }:

let
  # Auto-discover skill directories under paths.skills. Each subdirectory is a
  # Claude Code skill (a directory containing a SKILL.md, and optionally
  # assets/ and references/ subdirectories).
  entries = builtins.readDir paths.skills;

  skillNames = lib.sort lib.lessThan (
    lib.attrNames (lib.filterAttrs (_: type: type == "directory") entries)
  );

  sources = lib.listToAttrs (
    map (name: {
      inherit name;
      value = paths.skills + "/${name}";
    }) skillNames
  );

  allSkills = skillNames;

  antithesisSkills = builtins.filter (name: lib.hasPrefix "antithesis" name) allSkills;

  commonSkills = lib.subtractLists antithesisSkills allSkills;

  profiles = {
    antithesis = antithesisSkills;
    common = commonSkills;
  };

  mkSkillsDir =
    {
      pkgs,
      names ? allSkills,
      extraSkills ? { },
    }:
    let
      unknownNames = lib.subtractLists allSkills names;
      farm = pkgs.linkFarm "claude-skills-farm" (
        (map (name: {
          inherit name;
          path = sources.${name};
        }) names)
        ++ (lib.mapAttrsToList (name: path: { inherit name path; }) extraSkills)
      );
    in
    assert lib.assertMsg (
      unknownNames == [ ]
    ) "skills.mkSkillsDir contains unknown skills: ${lib.concatStringsSep ", " unknownNames}";
    # Claude Code's `/`-autocomplete discovery filters directory entries with
    # `Dirent.isFile()` and silently drops symlinks (anthropics/claude-code
    # issues #36659, #55791), so the published tree must be real directories of
    # real files. Dereference the link farm here in the sandbox, where every
    # symlink target is a store path, instead of asking each consumer to do it
    # on the host, and pin the no-symlink invariant so a future skill that
    # ships a symlink fails this build rather than vanishing from the menu.
    pkgs.runCommand "claude-skills" { } ''
      cp -RL ${farm} "$out"
      links=$(find "$out" -type l)
      if [ -n "$links" ]; then
        echo "claude-skills: symlinks survived materialization:" >&2
        echo "$links" >&2
        exit 1
      fi
    '';
in
{
  /**
    Skill source paths keyed by skill name.

    Each value is the path to a Claude Code skill directory (containing
    `SKILL.md`, and optionally `assets/` and `references/`). Discovered
    automatically from `paths.skills`, so adding a directory there is the
    only step needed to publish a new shared skill.
  */
  inherit sources;

  /**
    Sorted list of every discovered skill name.

    A consumer that wants everything can use `profiles.common ++
    profiles.antithesis`, which equals `allSkills`.
  */
  inherit allSkills;

  /**
    Curated skill subsets for consumers to pick from.

    `antithesis` is every skill whose name starts with `antithesis`;
    `common` is the rest. Together they partition `allSkills`.
  */
  inherit profiles;

  /**
    Build a single directory of selected skills for `.claude/skills`.

    Arguments:
    - `pkgs`: the package set used to build the skills directory.
    - `names`: skill names to include. Defaults to every discovered skill.
    - `extraSkills`: attrset from name to path for consumer-local skills
      that live outside this repository.

    Returns a directory holding one entry per skill (`<name>/` containing
    `SKILL.md`), built as real directories of real files with no symlinks:
    Claude Code's `/`-autocomplete discovery drops symlinked entries
    (anthropics/claude-code#36659), so deliver this by copying it into a
    repository's `.claude/skills` rather than symlinking the store path.
    Unknown names in `names` are rejected.
  */
  inherit mkSkillsDir;
}
