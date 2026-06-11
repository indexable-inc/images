{
  callPackage,
  git,
  pi-coding-agent,
  # Pinned bare `pi` binary (see ../shared/mk-pi-harness.nix). Override here
  # to swap the pi the wrapper execs.
  pi ? pi-coding-agent,
}:
let
  mkPiHarness = callPackage ../shared/mk-pi-harness.nix { inherit pi; };
  models = import ../shared/models.nix;
in
mkPiHarness {
  name = "pi-beam";
  description = "Pi executor with beam-search exploration over isolated worktree branches.";

  extensions = [ ./extension/beam.ts ];
  libFiles = [
    ./runner/fanout.js
    ../shared/ext-lib/scoring.js
  ];
  # Branch subprocesses load this by absolute path; it must NOT be auto-loaded
  # into the main executor (which is not turn-capped).
  auxFiles = [ ../shared/ext-lib/turn-cap.js ];

  inherit models;
  defaultModel = "claude";

  # Branches need the full tool surface to actually implement an approach.
  lockdown = false;
  session = true;

  runtimeInputs = [ git ];

  checkFiles = [ ./test/scoring.test.mjs ];
  checkLib = [ ../shared/ext-lib/scoring.js ];
}
