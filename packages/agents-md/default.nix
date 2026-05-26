{
  ix,
  lib,
  pkgs ? ix.pkgs,
}:

let
  unwrapped = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "agents-md";
    meta.mainProgram = "agents-md";
  };
  generatedDocuments = lib.listToAttrs (
    map (document: {
      name = document.target;
      value = pkgs.writeText document.fileName document.text;
    }) ix.agentsMd.documentList
  );
  documentsConfig = pkgs.writeText "agents-md-documents.json" (
    builtins.toJSON (
      map (document: {
        inherit (document) target;
        file_name = document.fileName;
        generated_path = "${generatedDocuments.${document.target}}";
      }) ix.agentsMd.documentList
    )
  );
  package =
    pkgs.runCommand "agents-md"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        strictDeps = true;
        meta = (unwrapped.meta or { }) // {
          description = "Diff, check, and write generated Codex and Claude instruction files";
          mainProgram = "agents-md";
        };
      }
      ''
        mkdir -p "$out/bin"
        makeWrapper ${unwrapped}/bin/agents-md "$out/bin/agents-md" \
          --set AGENTS_MD_DOCUMENTS ${documentsConfig} \
          --set AGENTS_MD_DELTA ${lib.getExe pkgs.delta}
      '';
in
package.overrideAttrs (old: {
  passthru =
    (old.passthru or { })
    // (unwrapped.passthru or { })
    // {
      inherit unwrapped;
    };
})
