{
  lib,
  buildIxRustTool,
  packagePath,
}: pkgs: {
  format,
  flavor ? "uncompressed",
}: let
  validFormats = [
    "nbt"
    "snbt"
  ];
  validFlavors = [
    "uncompressed"
    "gzip"
    "zlib"
  ];
  jsonFormat = pkgs.formats.json {};
  minecraftNbt = buildIxRustTool pkgs (packagePath "minecraft-nbt");
in
  assert lib.assertOneOf "mkMinecraftNbtFormat format" format validFormats;
  assert lib.assertOneOf "mkMinecraftNbtFormat flavor" flavor validFlavors; {
    inherit (jsonFormat) type;
    generate = name: value: let
      input = (pkgs.formats.json {}).generate "${name}.json" value;
    in
      pkgs.runCommand name {nativeBuildInputs = [minecraftNbt];} ''
        minecraft-nbt \
          --format ${lib.escapeShellArg format} \
          --flavor ${lib.escapeShellArg flavor} \
          --input ${input} \
          --output "$out"
      '';
  }
