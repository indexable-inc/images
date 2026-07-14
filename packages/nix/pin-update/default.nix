{
  ix,
  lib,
  formats,
  makeWrapper,
  runCommand,
  ...
}: let
  package = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "pin-update";
    meta.mainProgram = "pin-update";
    passthru = {inherit mkUpdateScript;};
  };

  # Builds an updater's `passthru.updateScript` entry: the pin-update binary
  # wrapped over a Nix-rendered mode-tagged JSON spec (never a hand-assembled
  # argv string) plus the PATH its mode shells out to. User arguments (the
  # claude-code mode's version / --prompts-only) pass through after the baked
  # spec.
  mkUpdateScript = {
    name,
    description,
    spec,
    runtimeInputs ? [],
  }: let
    specFile = (formats.json {}).generate "${name}-spec.json" spec;
    pathArgs = lib.optionalString (runtimeInputs != []) " --prefix PATH : ${lib.makeBinPath runtimeInputs}";
  in
    runCommand name
    {
      __structuredAttrs = true;
      nativeBuildInputs = [makeWrapper];
      meta = {
        inherit description;
        mainProgram = name;
      };
    }
    ''
      # shell
      makeWrapper ${lib.getExe package} "$out/bin/${name}" --add-flags ${specFile}${pathArgs}
    '';
in
  # selectBinaryWithTests keeps `passthru` nested; lift the builder so
  # consumers write `pinUpdate.mkUpdateScript` like a mkDerivation passthru.
  package // {inherit mkUpdateScript;}
