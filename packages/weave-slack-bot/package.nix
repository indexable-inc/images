{
  id = "weave-slack-bot";
  packageSet = true;
  flake.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  overlay = false;
}
