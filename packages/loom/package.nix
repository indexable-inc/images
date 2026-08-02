{
  id = "loom";
  packageSet = {
    systems = ["x86_64-linux"];
  };
  flake = {
    systems = ["x86_64-linux"];
  };
  passthruTests = {
    prefix = "loom";
  };
}
