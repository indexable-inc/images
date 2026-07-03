{
  callPackage,
  git,
  ix,
  pi-coding-agent,
  # Pinned bare pi binary (see the shared mk-pi-harness.nix). Override here
  # to swap the pi the wrapper execs.
  pi ? pi-coding-agent,
}: let
  shared = ix.paths.packagesRoot + "/agent/pi-harnesses/shared";
  mkPiHarness = callPackage (shared + "/mk-pi-harness.nix") {inherit pi;};
  models = import (shared + "/models.nix");
in
  mkPiHarness {
    name = "pi-fusion";
    description = "Pi primary agent with a delegated gpt-5.5-low sidekick.";

    extensions = [./extension/fusion.ts];
    libFiles = [
      ./runner/sidekick.js
    ];

    inherit models;
    defaultModel = "fable";

    # The primary keeps a limited-but-real tool surface: it delegates bulk work,
    # monitors results, and applies/rejects patches.
    lockdown = false;
    session = true;

    runtimeInputs = [git];

    checkFiles = [./test/sidekick.test.mjs];
    checkLib = [./runner/sidekick.js];
  }
