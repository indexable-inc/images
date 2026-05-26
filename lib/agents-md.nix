{ lib, paths }:

let
  section = name: file: {
    inherit name;
    text = builtins.readFile (paths.agentsMd + "/sections/${file}");
  };

  sectionList = [
    (section "intro" "00-intro.md")
    (section "scope" "01-scope-of-agents-md.md")
    (section "workflow" "02-workflow.md")
    (section "siteUpdates" "03-site-updates.md")
    (section "writingStyle" "04-writing-style.md")
    (section "inlineComments" "05-inline-comments.md")
    (section "rustStyle" "06-rust-style.md")
    (section "pythonStyle" "07-python-style.md")
    (section "saneDefaults" "08-sane-defaults.md")
    (section "nixPhilosophy" "09-nix-philosophy.md")
    (section "moduleConventions" "10-module-conventions.md")
    (section "imageConventions" "11-image-conventions.md")
    (section "layout" "12-layout.md")
    (section "dependencyIntake" "13-dependency-intake.md")
    (section "nixPractices" "14-nix-practices-to-tighten.md")
    (section "nixStyle" "15-nix-style-ast-grep-enforced.md")
    (section "issues" "16-issues.md")
    (section "tests" "17-tests.md")
    (section "searching" "18-searching.md")
    (section "debuggingVms" "19-debugging-vms.md")
    (section "linting" "20-linting.md")
  ];

  sections = lib.listToAttrs (
    map (fragment: {
      inherit (fragment) name;
      value = fragment.text;
    }) sectionList
  );

  profiles = {
    common = [
      "scope"
      "workflow"
      "writingStyle"
      "inlineComments"
      "rustStyle"
      "pythonStyle"
      "saneDefaults"
      "dependencyIntake"
      "issues"
      "tests"
      "searching"
    ];

    nix = [
      "nixPhilosophy"
      "nixPractices"
      "nixStyle"
      "linting"
    ];

    index = [
      "intro"
      "siteUpdates"
      "moduleConventions"
      "imageConventions"
      "layout"
      "debuggingVms"
    ];
  };

  allSections = map (fragment: fragment.name) sectionList;

  render =
    {
      enabledSections ? allSections,
      extraSections ? [ ],
    }:
    let
      unknownSections = lib.subtractLists allSections enabledSections;
    in
    assert lib.assertMsg (unknownSections == [ ])
      "agentsMd.render enabledSections contains unknown sections: ${lib.concatStringsSep ", " unknownSections}";
    lib.concatStrings ((map (name: sections.${name}) enabledSections) ++ extraSections);
in
{
  /**
    Named AGENTS.md fragments keyed by stable section names.

    Each value is Markdown text including its heading. Use these fragments
    when a repository needs to build a custom `AGENTS.md` without copying
    the whole ix/images file.
  */
  inherit sections;

  /**
    Ordered section groups for common reuse.

    `common` is repo-agnostic process and style guidance, `nix` is reusable
    Nix guidance, and `index` is ix/images-specific material.
  */
  inherit profiles;

  /**
    Render an AGENTS.md document from ordered section names.

    Arguments:
    - `enabledSections`: ordered list of keys from `sections`. Defaults to
      every section, which reproduces this repository's checked-in file.
    - `extraSections`: extra Markdown strings appended after the enabled
      sections for local repository notes.
  */
  inherit render;
}
