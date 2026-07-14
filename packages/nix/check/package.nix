{
  id = "check";
  # x86_64-linux only: `check` is the CI gate for the system CI builds for,
  # and its wrapper closes over the linux-built evaluators.
  packageSet = {systems = ["x86_64-linux"];};
  flake = {systems = ["x86_64-linux"];};
  overlay = false;
  inRustWorkspace = true;
  passthruTests = true;
}
