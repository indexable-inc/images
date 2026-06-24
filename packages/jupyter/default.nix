{
  ix,
  lib,
  pkgs,
  repoPackages,
}:
let
  nu-jupyter-kernel = repoPackages.nu-jupyter-kernel;
  python = pkgs.python3.withPackages (ps: [
    ps.jupyterlab
    ps.notebook
  ]);
in
ix.writeBashApplication pkgs {
  name = "jupyter";
  runtimeInputs = [
    python
    nu-jupyter-kernel
  ];
  text = ''
    kernel_dir="''${XDG_DATA_HOME:-$HOME/.local/share}/jupyter/kernels/nu"
    mkdir -p "$kernel_dir"
    cat > "$kernel_dir/kernel.json" <<KERNEL
    {
      "argv": ["${lib.getExe nu-jupyter-kernel}", "--connection-file", "{connection_file}"],
      "display_name": "Nushell",
      "language": "nushell"
    }
    KERNEL

    echo "Nushell kernel registered."
    exec jupyter lab --no-browser "$@"
  '';
}
