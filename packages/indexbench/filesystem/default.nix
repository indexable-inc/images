{
  ix,
  lib,
  symlinkJoin,
  makeWrapper,
  fio,
  ...
}: let
  bin = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "ix-bench-filesystem";
    packageName = "indexbench-filesystem";
    meta = {
      mainProgram = "ix-bench-filesystem";
      description = "Benchmark file-system behavior from inside an ix VM";
    };
  };
in
  symlinkJoin {
    name = "ix-bench-filesystem";
    paths = [bin];
    nativeBuildInputs = [makeWrapper];

    # fio is the one external tool the benchmark drives; everything else the
    # former Nushell script shelled out for (mktemp, sync, stat, find, uname)
    # is native Rust now.
    postBuild = ''
      # shell
      wrapProgram $out/bin/ix-bench-filesystem \
        --prefix PATH : ${lib.makeBinPath [fio]}
    '';

    inherit (bin) meta;
    # Drop `unchecked` so consumers of `passthru` never unwrap back to the
    # bare, fio-less binary (same reasoning as packages/minecraft/sound).
    passthru = builtins.removeAttrs bin.passthru ["unchecked"];
  }
