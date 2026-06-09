{
  callPackage,
  git,
  pi ? null,
}:
let
  mkPiHarness = callPackage ../shared/mk-pi-harness.nix { inherit pi; };
  models = import ../shared/models.nix;
in
mkPiHarness {
  name = "pi-base";
  description = "Pi with the base UX pack: live tok/s, git status widget, /diff, /lg.";

  extensions = [
    ./extension/banner.ts
    ./extension/tps-tracker.ts
    ./extension/git-status-widget.ts
    ./extension/turn-diff.ts
    ./extension/lg.ts
  ];
  libFiles = [ ../shared/ext-lib/git-files.js ];

  inherit models;
  defaultModel = "claude";

  lockdown = false;
  session = true;

  runtimeInputs = [ git ];

  checkFiles = [ ./test/git-files.test.mjs ];
  checkLib = [ ../shared/ext-lib/git-files.js ];
}
