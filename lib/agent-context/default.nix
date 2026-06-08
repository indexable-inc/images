{ lib, paths }:
/**
  Agent context: the instructions and skills delivered to a coding agent at
  session start.

  Each file under `agent-context/sections/` carries YAML frontmatter naming the
  section and declaring its disclosure tier:

  - `disclosure: always` sections are concatenated into one small always-on
    document (`alwaysDoc`) that the SessionStart hook prints in full.
  - `disclosure: progressive` sections each become a Claude Code skill
    (`mkProgressiveSkills`): only the `name` + `description` stay always-visible,
    and the body loads on demand when the skill is invoked.

  This keeps always-on context small. The total size of the always tier is a
  build-time invariant (`alwaysCharCap`): marking too much `always` fails the
  build instead of silently overflowing Claude's per-value context limit.
*/
let
  parseFrontmatter = import ./frontmatter.nix { inherit lib; };

  sectionsDir = paths.agentContext + "/sections";

  # Claude Code replaces a single SessionStart `additionalContext` value larger
  # than 10000 chars with a ~2KB preview, silently dropping the rest. We hold a
  # safety margin below that and make the limit a checked invariant rather than a
  # silent truncation.
  alwaysCharCap = 9000;

  sectionFileNames = lib.sort lib.lessThan (
    lib.attrNames (
      lib.filterAttrs (name: type: type == "regular" && lib.hasSuffix ".md" name) (
        builtins.readDir sectionsDir
      )
    )
  );

  parseSection =
    fileName:
    let
      parsed = parseFrontmatter (builtins.readFile (sectionsDir + "/${fileName}"));
      fm = parsed.frontmatter;
      name = fm.name or (throw "agent-context: ${fileName} is missing `name` in its frontmatter");
      disclosure =
        fm.disclosure
          or (throw "agent-context: ${fileName} (${name}) is missing `disclosure` (always|progressive)");
    in
    assert lib.assertMsg (disclosure == "always" || disclosure == "progressive")
      "agent-context: ${fileName} (${name}) has disclosure='${disclosure}', expected 'always' or 'progressive'";
    assert lib.assertMsg (disclosure != "progressive" || (fm.description or "") != "")
      "agent-context: progressive section ${name} (${fileName}) needs a non-empty `description`: it is the trigger Claude uses to decide when to load the skill";
    {
      inherit name disclosure fileName;
      description = fm.description or "";
      inherit (parsed) body;
    };

  # Ordered by file name so the rendered document keeps the section sequence.
  parsedSections = map parseSection sectionFileNames;

  duplicateNames =
    let
      names = map (section: section.name) parsedSections;
    in
    lib.subtractLists (lib.unique names) names;

  checkedSections =
    assert lib.assertMsg (
      duplicateNames == [ ]
    ) "agent-context: duplicate section names: ${lib.concatStringsSep ", " duplicateNames}";
    parsedSections;

  alwaysSections = lib.filter (section: section.disclosure == "always") checkedSections;
  progressiveSections = lib.filter (section: section.disclosure == "progressive") checkedSections;

  alwaysDocBody =
    lib.concatStringsSep "\n\n" (
      map (section: lib.trimWith { end = true; } section.body) alwaysSections
    )
    + "\n";

  alwaysDocLength = lib.stringLength alwaysDocBody;

  alwaysDoc =
    assert lib.assertMsg (alwaysDocLength < alwaysCharCap)
      "agent-context: always-on instructions are ${toString alwaysDocLength} chars, over the ${toString alwaysCharCap} cap. Mark a section `disclosure: progressive` so it loads as an on-demand skill instead of always-on context.";
    alwaysDocBody;

  # SKILL.md text for a progressive section: minimal `name` + `description`
  # frontmatter (the always-visible trigger) followed by the section body.
  # `toJSON` produces a quoted scalar valid as YAML, so colons and quotes in the
  # description survive.
  skillText =
    section:
    ''
      ---
      name: ${section.name}
      description: ${builtins.toJSON section.description}
      ---
    ''
    + "\n"
    + lib.trimWith { end = true; } section.body
    + "\n";

  documents = [
    {
      target = "claude";
      fileName = "CLAUDE.md";
      text = alwaysDoc;
    }
    {
      target = "codex";
      fileName = "AGENTS.md";
      text = alwaysDoc;
    }
  ];
in
{
  /**
    Hard upper bound (chars) on the always-on document. See `alwaysDoc`.
  */
  inherit alwaysCharCap;

  /**
    Parsed sections keyed by frontmatter `name`. Each value is
    `{ name, disclosure, description, fileName, body }`.
  */
  sections = lib.genAttrs' checkedSections (section: {
    inherit (section) name;
    value = section;
  });

  /**
    Sections in each tier, in file order.
  */
  inherit alwaysSections progressiveSections;

  /**
    The always-on instruction document: every `disclosure: always` section
    concatenated, in file order. Forcing this value asserts it stays under
    `alwaysCharCap`; the SessionStart hook prints it as one `additionalContext`
    value.
  */
  inherit alwaysDoc alwaysDocLength;

  /**
    The always-on document rendered per instruction target. Both targets share
    the same core today; the list shape mirrors the CLI's write/check order.
  */
  inherit documents;

  /**
    Build the progressive sections into skill directories.

    Returns an attrset from skill name to a derivation whose output is a
    directory containing `SKILL.md`. Pass it as `extraSkills` to
    `skills.mkSkillsDir` to merge generated section-skills with the handwritten
    ones under a single `.claude/skills` directory.

    Arguments:
    - `pkgs`: the package set used to write the skill files.
  */
  mkProgressiveSkills =
    { pkgs }:
    lib.genAttrs' progressiveSections (section: {
      inherit (section) name;
      value = pkgs.writeTextDir "SKILL.md" (skillText section);
    });

  /**
    Wrap the `agents-md` Rust CLI so contributors can write or diff the
    always-on instruction files on disk (`nix run .#agent-context -- --write`).
    The committed files are not tracked; this is a preview convenience.

    Arguments:
    - `pkgs`: the package set used to build the wrapper.
    - `binary`: the unwrapped `agents-md` binary derivation.
  */
  mkApp =
    { pkgs, binary }:
    let
      generatedDocuments = lib.genAttrs' documents (document: {
        name = document.target;
        value = pkgs.writeText document.fileName document.text;
      });
      documentsConfig = (pkgs.formats.json { }).generate "agent-context-documents.json" (
        map (document: {
          inherit (document) target;
          file_name = document.fileName;
          generated_path = "${generatedDocuments.${document.target}}";
        }) documents
      );
    in
    pkgs.runCommand "agent-context"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        strictDeps = true;
        meta = (binary.meta or { }) // {
          description = "Diff, check, and write generated Codex and Claude instruction files";
          mainProgram = "agents-md";
        };
        passthru = (binary.passthru or { }) // {
          inherit documentsConfig;
          documentList = documents;
        };
      }
      ''
        mkdir -p "$out/bin"
        makeWrapper ${binary}/bin/agents-md "$out/bin/agents-md" \
          --set AGENTS_MD_DOCUMENTS ${documentsConfig} \
          --set AGENTS_MD_DELTA ${lib.getExe pkgs.delta}
      '';
}
