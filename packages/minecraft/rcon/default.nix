{
  pkgs,
  writePythonApplication,
}:
writePythonApplication {
  name = "minecraft-rcon";
  src = ./minecraft-rcon.py;
  pyChecker = "zuban";
  python = pkgs.python314.withPackages (ps: [ps.pydantic-settings]);
}
