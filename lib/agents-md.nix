{ lib, paths }:

let
  section = name: file: {
    inherit name;
    text = builtins.readFile (paths.agentsMd + "/sections/${file}");
  };

  sectionList = [
    (section "intro" "00-intro.md")
    (section "scope" "01-scope-of-agents-md.md")
    (section "craft" "01b-craft-standard.md")
    (section "workflow" "02-workflow.md")
    (section "indexGitPush" "02a-index-git-push.md")
    (section "siteUpdates" "03-site-updates.md")
    (section "writingStyle" "04-writing-style.md")
    (section "replyShape" "04b-reply-shape.md")
    (section "inlineComments" "05-inline-comments.md")
    (section "rustStyle" "06-rust-style.md")
    (section "pythonStyle" "07-python-style.md")
    (section "saneDefaults" "08-sane-defaults.md")
    (section "userFacingCommands" "08a-user-facing-commands.md")
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
      "craft"
      "workflow"
      "writingStyle"
      "replyShape"
      "inlineComments"
      "rustStyle"
      "pythonStyle"
      "saneDefaults"
      "userFacingCommands"
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
      "indexGitPush"
      "siteUpdates"
      "moduleConventions"
      "imageConventions"
      "layout"
      "debuggingVms"
    ];
  };

  allSections = map (fragment: fragment.name) sectionList;

  targetList = [
    {
      name = "codex";
      fileName = "AGENTS.md";
      sections = [ ];
    }
    {
      name = "claude";
      fileName = "CLAUDE.md";
      sections = [ ];
    }
  ];

  targets = lib.listToAttrs (
    map (target: {
      inherit (target) name;
      value = removeAttrs target [ "name" ];
    }) targetList
  );

  stripTrailingNewlines =
    text: if lib.hasSuffix "\n" text then stripTrailingNewlines (lib.removeSuffix "\n" text) else text;

  render =
    {
      enabledSections ? allSections,
      extraSections ? [ ],
      extraSectionsByTarget ? { },
      target ? null,
      targetSections ? { },
    }:
    let
      targetConfig =
        if target == null then
          { sections = [ ]; }
        else if builtins.hasAttr target targets then
          targets.${target}
        else
          throw "agentsMd.render target must be one of: ${lib.concatStringsSep ", " (lib.attrNames targets)}";
      selectedTargetSections =
        targetConfig.sections ++ (if target == null then [ ] else targetSections.${target} or [ ]);
      selectedSections = enabledSections ++ selectedTargetSections;
      selectedExtraSections =
        extraSections ++ (if target == null then [ ] else extraSectionsByTarget.${target} or [ ]);
      unknownSections = lib.subtractLists allSections selectedSections;
    in
    assert lib.assertMsg (
      unknownSections == [ ]
    ) "agentsMd.render contains unknown sections: ${lib.concatStringsSep ", " unknownSections}";
    lib.concatStringsSep "\n\n" (
      map stripTrailingNewlines ((map (name: sections.${name}) selectedSections) ++ selectedExtraSections)
    )
    + "\n";

  documentListFor =
    renderArgs:
    map (target: {
      target = target.name;
      inherit (target) fileName;
      text = render (renderArgs // { target = target.name; });
    }) targetList;

  documentList = documentListFor { };

  documents = lib.listToAttrs (
    map (document: {
      name = document.target;
      value = removeAttrs document [ "target" ];
    }) documentList
  );

  mkApp =
    {
      pkgs,
      binary,
      enabledSections ? allSections,
      extraSections ? [ ],
      extraSectionsByTarget ? { },
      targetSections ? { },
    }:
    let
      customDocumentList = documentListFor {
        inherit
          enabledSections
          extraSections
          extraSectionsByTarget
          targetSections
          ;
      };
      generatedDocuments = lib.listToAttrs (
        map (document: {
          name = document.target;
          value = pkgs.writeText document.fileName document.text;
        }) customDocumentList
      );
      documentsConfig = (pkgs.formats.json { }).generate "agents-md-documents.json" (
        map (document: {
          inherit (document) target;
          file_name = document.fileName;
          generated_path = "${generatedDocuments.${document.target}}";
        }) customDocumentList
      );
    in
    pkgs.runCommand "agents-md"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        strictDeps = true;
        meta = (binary.meta or { }) // {
          description = "Diff, check, and write generated Codex and Claude instruction files";
          mainProgram = "agents-md";
        };
        passthru = (binary.passthru or { }) // {
          inherit documentsConfig;
          documentList = customDocumentList;
        };
      }
      ''
        mkdir -p "$out/bin"
        makeWrapper ${binary}/bin/agents-md "$out/bin/agents-md" \
          --set AGENTS_MD_DOCUMENTS ${documentsConfig} \
          --set AGENTS_MD_DELTA ${lib.getExe pkgs.delta}
      '';
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
    Instruction targets generated by this repository.

    Codex reads `AGENTS.md`; Claude reads `CLAUDE.md`. A target can append
    named sections through its `sections` list, and callers can pass
    `targetSections.<target>` to `render` for consumer-specific additions.
  */
  inherit targets;

  /**
    Rendered repository instruction documents keyed by target.

    Each document has `{ fileName, text }` so tooling can write or validate
    every generated instruction file without duplicating target knowledge.
  */
  inherit documents;

  /**
    Rendered repository instruction documents in write/check order.
  */
  inherit documentList;

  /**
    Render an AGENTS.md document from ordered section names.

    Arguments:
    - `enabledSections`: ordered list of keys from `sections`. Defaults to
      every section, which reproduces this repository's checked-in file.
    - `extraSections`: extra Markdown strings appended after the enabled
      sections for local repository notes.
    - `extraSectionsByTarget`: attrset from target name to target-only
      Markdown strings.
    - `target`: optional instruction target, such as `codex` or `claude`.
    - `targetSections`: attrset from target name to extra named section keys.
  */
  inherit render;

  /**
    Wrap the `agents-md` CLI with a caller-specific document set.

    Consumer repositories pass the unwrapped binary from
    `index.packages.<system>.agents-md.passthru.unwrapped` plus their own
    render arguments and get back a `nix run`-able package that writes the
    consumer's `AGENTS.md` and `CLAUDE.md`. Arguments after `binary` mirror
    `render` and are applied to every target.
  */
  inherit mkApp;
}
