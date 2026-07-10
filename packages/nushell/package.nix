{
  id = "nushell";
  packageSet = true;
  flake = true;
  overlay = false;
  cross = {
    exposeNativeDarwin = false;
  };
  updateScript = true;
}
