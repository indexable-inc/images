{
  ix,
  pkgs,
}:

ix.writeNushellApplication pkgs {
  name = "ix-bench-filesystem";
  runtimeInputs = [
    pkgs.coreutils
    pkgs.findutils
    pkgs.fio
  ];
  text = builtins.readFile ./run.nu;
  meta.description = "Benchmark file-system behavior from inside an ix VM";
}
