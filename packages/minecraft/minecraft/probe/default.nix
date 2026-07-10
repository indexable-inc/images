{
  ix,
  pkgs ? ix.pkgs,
}: let
  # mc-probe imports `mc_protocol`, the unibind-rendered bindings of the Rust
  # mc-protocol crate, so the probe and the servers' tests speak the wire
  # format through one implementation. Same arguments as
  # packages/minecraft/minecraft/protocol/py/default.nix (the wheel); keep the
  # two call sites in sync.
  mcProtocolModule =
    (ix.unibind.build {
      crate = "mc-protocol-py";
      targets.py = {
        package = "mc_protocol";
        pythonSource = builtins.path {
          name = "mc-protocol-py-python-source";
          path = ix.paths.packagesRoot + "/minecraft/minecraft/protocol/py/python";
        };
      };
    }).py.module;
in
  ix.writePythonApplication pkgs {
    name = "mc-probe";
    src = ./mc_probe.py;
    pyChecker = "zuban";
    python = pkgs.python3.withPackages (_ps: [mcProtocolModule]);
    meta.description = "Assert Minecraft Server List Ping responses";
  }
