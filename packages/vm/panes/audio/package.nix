{
  id = "panes-audio";
  inRustWorkspace = true;
  # Guest-side: runs inside the aarch64-linux VM. x86_64-linux is in the set
  # because the crate is arch-generic Linux and CI's flake-check builds only
  # the x86_64-linux graph (check.yml): without it, nothing would compile,
  # lint, or test this crate pre-merge (unlike panes-compositor, whose
  # aarch64-only smithay stack is validated on the local builder instead).
  flake.systems = [
    "aarch64-linux"
    "x86_64-linux"
  ];
  packageSet.systems = [
    "aarch64-linux"
    "x86_64-linux"
  ];
  passthruTests = true;
}
