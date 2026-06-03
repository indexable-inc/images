{ ix, lib, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "skill-lint";
  meta = {
    description = "Lint and autofix SKILL.md files with real YAML frontmatter parsing";
    license = lib.licenses.mit;
    mainProgram = "skill-lint";
  };
}
