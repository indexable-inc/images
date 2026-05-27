{
  lib,
  buildIxRustTool,
  packagePath,
}:
pkgs:
{
  format,
  flavor ? "uncompressed",
}:
let
  validFormats = [
    "nbt"
    "snbt"
  ];
  validFlavors = [
    "uncompressed"
    "gzip"
    "zlib"
  ];
  jsonFormat = pkgs.formats.json { };
  minecraftNbt = buildIxRustTool pkgs (packagePath "minecraft-nbt");
in
assert lib.assertMsg (builtins.elem format validFormats)
  "mkMinecraftNbtFormat: format must be one of ${lib.concatStringsSep ", " validFormats}";
assert lib.assertMsg (builtins.elem flavor validFlavors)
  "mkMinecraftNbtFormat: flavor must be one of ${lib.concatStringsSep ", " validFlavors}";
{
  inherit (jsonFormat) type;
  generate =
    name: value:
    let
      input = pkgs.writeText "${name}.json" (builtins.toJSON value);
    in
    pkgs.runCommand name { nativeBuildInputs = [ minecraftNbt ]; } ''
      minecraft-nbt \
        --format ${lib.escapeShellArg format} \
        --flavor ${lib.escapeShellArg flavor} \
        --input ${input} \
        --output "$out"
    '';
}
