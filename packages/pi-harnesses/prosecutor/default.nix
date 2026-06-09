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
  name = "pi-prosecutor";
  description = "Pi executor under a skeptical prosecutor with earned-trust check-ins.";

  extensions = [ ./extension/prosecutor.ts ];
  libFiles = [
    ../shared/ext-lib/trust.js
    ../shared/ext-lib/child-agent.js
    ../shared/ext-lib/probes.js
  ];

  inherit models;
  defaultModel = "claude";

  # The executor needs its real tool surface to do work, and the isolated
  # prosecutor needs built-in tools to probe ground truth. This is the opposite
  # posture from the engine harness, which strips tools for the Room sandbox.
  lockdown = false;
  session = true;

  # The prosecutor reuses the active executor model (opus-4-8 / gpt-5.5 medium)
  # by default - context isolation, not a weaker model, is what stops the two
  # agents laundering each other's hallucinations. Override per-run with
  # PI_PROSECUTOR_PROVIDER / PI_PROSECUTOR_MODEL / PI_PROSECUTOR_THINKING.

  runtimeInputs = [ git ];

  checkFiles = [ ./test/trust.test.mjs ];
  checkLib = [ ../shared/ext-lib/trust.js ];
}
