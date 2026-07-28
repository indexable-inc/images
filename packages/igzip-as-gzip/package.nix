{
  id = "igzip-as-gzip";
  packageSet = true;
  flake.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  overlay = true;
}
