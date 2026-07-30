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

  # Ingest each skill as its own store path instead of leaving it as a subpath
  # of `paths.skills`, because a subpath carries the whole enclosing tree as
  # its source identity. `paths.skills` is a relative-path input whose lock
  # node resolves against the PARENT tree, so a consumer vendoring index as
  # `path:./index` (ix does) sees
  # `<consumer-tree>/index/packages/agent/skills/<name>` and re-keys every
  # skill on every commit anywhere in that repo. The reach is the whole fleet:
  # skills -> claude-code's launch spec -> claude-code -> system-path -> each
  # host's toplevel, so one comment in ix's docs/ moved all twelve ix hosts'
  # toplevel drvPaths (ENG-11432). Ingesting per skill keys each entry on that
  # skill's own bytes, so a commit touching no skill leaves all of them
  # identical. `mkSkillsDir` asserts the property; do not return to a bare
  # `paths.skills + "/${name}"`.
  ingest = name: path:
    builtins.path {
      inherit path;
      name = "claude-skill-${name}";
    };

  sources = lib.genAttrs skillNames (name: ingest name (paths.skills + "/${name}"));

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

  vendorCollisions = lib.intersectLists skillNames vendoredNames;

  allSkills = assert lib.assertMsg (vendorCollisions == [])
  "skills: vendored skill name(s) shadow repo skills: ${lib.concatStringsSep ", " vendorCollisions}";
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
    repoEntries =
      map (name: {
        inherit name;
        path = sources.${name} or (vendoredSources.${name} pkgs);
      })
      names;
    # Consumer-local skills get the same per-skill ingestion `sources` gets, so
    # a consumer handing a repo-relative path (ix hands `<ix-tree>/skills/...`)
    # does not couple its skills directory to every commit in its own repo.
    extraEntries =
      lib.mapAttrsToList (name: path: {
        inherit name;
        path = ingest name path;
      })
      extraSkills;
    farm = pkgs.linkFarm "claude-skills-farm" (repoEntries ++ extraEntries);
    # Every entry must be its own store root, not a subpath of an enclosing
    # tree: see `ingest` for what a subpath costs the fleet. Vendored skills are
    # exempt because they are subpaths of their own package's output, which
    # already moves only when that package does. Forced on every skills build,
    # so an edit that drops the ingestion fails here rather than silently
    # re-coupling claude-code to every commit in the consuming repo.
    unscoped = map (entry: entry.name) (
      lib.filter
      (entry:
        !(lib.elem entry.name vendoredNames)
        && builtins.dirOf (toString entry.path) != builtins.storeDir)
      (repoEntries ++ extraEntries)
    );
  in
    assert lib.assertMsg (
      unknownNames == []
    ) "skills.mkSkillsDir contains unknown skills: ${lib.concatStringsSep ", " unknownNames}";
    assert lib.assertMsg (
      extraCollisions == []
    ) "skills.mkSkillsDir: extraSkills name(s) collide with index skills: ${lib.concatStringsSep ", " extraCollisions}";
    assert lib.assertMsg (
      unscoped == []
    ) "skills.mkSkillsDir: skill path(s) are a subpath of an enclosing tree, so every commit in the consuming repo re-keys this directory and everything downstream of it: ${lib.concatStringsSep ", " unscoped}";
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

  Each value is the store path of one Claude Code skill directory
  (containing `SKILL.md`, and optionally `assets/` and `references/`),
  ingested per skill so it is keyed on that skill's own bytes rather than on
  the tree it was discovered in (see `ingest`). Discovered automatically
  from `paths.skills`, so adding a directory there is the only step needed
  to publish a new shared skill. Vendored skills are not here: their paths
  depend on the consumer's package set, so `mkSkillsDir` resolves them from
  `vendoredSources`.
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
    that live outside this repository. Values must be source paths, not
    derivation outputs: each is ingested per skill (see `ingest`), and
    ingesting a derivation output would force a build during evaluation.

  Returns a directory holding one entry per skill (`<name>/` containing
  `SKILL.md`), built as real directories of real files with no symlinks:
  Claude Code's `/`-autocomplete discovery drops symlinked entries
  (anthropics/claude-code#36659), so deliver this by copying it into a
  repository's `.claude/skills` rather than symlinking the store path.
  Unknown names in `names` are rejected.
  */
  inherit mkSkillsDir;
}
