{
  lib,
  paths,
}: let
  # Auto-discover skill directories under paths.skills. Each subdirectory is a
  # Claude Code skill (a directory containing a SKILL.md, and optionally
  # assets/ and references/ subdirectories). `vendoredSources` below adds
  # skills that ship inside packaged upstreams.
  entries = builtins.readDir paths.skills;

  skillNames = lib.sort lib.lessThan (
    lib.attrNames (lib.filterAttrs (_: type: type == "directory") entries)
  );

  sources = lib.genAttrs skillNames (name: paths.skills + "/${name}");

  # Skills vendored from packaged upstreams: the package already ships a
  # plugin-ready skill directory, so the catalog derives it from the pin
  # instead of committing a copy that drifts from the installed binary.
  # Values are functions of the consumer's package set (this file has no
  # `pkgs`), resolved inside `mkSkillsDir`. agent-browser's directory is
  # upstream's discovery stub: it sends the agent to
  # `agent-browser skills get core`, which the CLI serves byte-matched to
  # its own version, so the stub itself never goes stale.
  vendoredSources = {
    agent-browser = pkgs: "${pkgs.agent-browser}/skills/agent-browser";
  };

  vendoredNames = lib.attrNames vendoredSources;

  # The same names again as plain data, for the one consumer that cannot
  # evaluate Nix: the SessionStart materializer in
  # `.claude/hooks/agent-instructions.sh`. That hook copies the checkout's own
  # `paths.skills` over the prebuilt store copy the Claude wrapper baked
  # (ENG-11189), which leaves it deciding, for each entry the store carries and
  # the tree does not, whether it is a skill that comes from a package (keep) or
  # one this checkout has deleted since the wrapper was built (drop). Only the
  # keys are mirrored, because the values are functions of the consumer's
  # package set; the assert below is what stops the mirror going stale, so a
  # vendored skill added to `vendoredSources` alone fails every eval instead of
  # silently vanishing from `.claude/skills` in every checkout.
  manifestNames =
    lib.filter (name: name != "" && !lib.hasPrefix "#" name)
    (lib.splitString "\n" (builtins.readFile (paths.skills + "/vendored-skills.txt")));

  vendorCollisions = lib.intersectLists skillNames vendoredNames;

  allSkills = assert lib.assertMsg (vendorCollisions == [])
  "skills: vendored skill name(s) shadow repo skills: ${lib.concatStringsSep ", " vendorCollisions}";
  assert lib.assertMsg (manifestNames == vendoredNames)
  "skills: packages/agent/skills/vendored-skills.txt lists [${lib.concatStringsSep ", " manifestNames}] but vendoredSources defines [${lib.concatStringsSep ", " vendoredNames}]; the SessionStart materializer reads the file, so they must match";
    lib.sort lib.lessThan (skillNames ++ vendoredNames);

  partitioned = lib.partition (lib.hasPrefix "antithesis") allSkills;

  antithesisSkills = partitioned.right;

  commonSkills = partitioned.wrong;

  profiles = {
    antithesis = antithesisSkills;
    common = commonSkills;
  };

  mkSkillsDir = {
    pkgs,
    names ? allSkills,
    extraSkills ? {},
  }: let
    unknownNames = lib.subtractLists allSkills names;
    # Without this a clash surfaces as a linkFarm duplicate-entry build
    # failure naming neither side. Same guard claudePlugin.mkPlugin carries.
    extraCollisions = lib.intersectLists names (lib.attrNames extraSkills);
    farm = pkgs.linkFarm "claude-skills-farm" (
      (map (name: {
          inherit name;
          path = sources.${name} or (vendoredSources.${name} pkgs);
        })
        names)
      ++ (lib.mapAttrsToList (name: path: {inherit name path;}) extraSkills)
    );
  in
    assert lib.assertMsg (
      unknownNames == []
    ) "skills.mkSkillsDir contains unknown skills: ${lib.concatStringsSep ", " unknownNames}";
    assert lib.assertMsg (
      extraCollisions == []
    ) "skills.mkSkillsDir: extraSkills name(s) collide with index skills: ${lib.concatStringsSep ", " extraCollisions}";
    # Claude Code's `/`-autocomplete discovery filters directory entries with
    # `Dirent.isFile()` and silently drops symlinks (anthropics/claude-code
    # issues #36659, #55791), so the published tree must be real directories of
    # real files. Dereference the link farm here in the sandbox, where every
    # symlink target is a store path, instead of asking each consumer to do it
    # on the host, and pin the no-symlink invariant so a future skill that
    # ships a symlink fails this build rather than vanishing from the menu.
      pkgs.runCommand "claude-skills" {} ''
        cp -RL ${farm} "$out"
        links=$(find "$out" -type l)
        if [ -n "$links" ]; then
          echo "claude-skills: symlinks survived materialization:" >&2
          echo "$links" >&2
          exit 1
        fi
      '';
in {
  /**
  Skill source paths keyed by skill name.

  Each value is the path to a Claude Code skill directory (containing
  `SKILL.md`, and optionally `assets/` and `references/`). Discovered
  automatically from `paths.skills`, so adding a directory there is the
  only step needed to publish a new shared skill. Vendored skills are not
  here: their paths depend on the consumer's package set, so `mkSkillsDir`
  resolves them from `vendoredSources`.
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
