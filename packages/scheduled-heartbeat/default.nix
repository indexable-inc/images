{
  ix,
  pkgs ? ix.pkgs,
}: let
  scripts = ix.paths.root + "/.github/scripts";
  module = scripts + "/scheduled-heartbeat.mjs";
  suite = scripts + "/scheduled-heartbeat.test.mjs";
in
  # The module that decides whether a cron has stopped firing, packaged so its
  # tests have somewhere to hang.
  #
  # A heartbeat is worth exactly what its accuracy is worth: one that reports
  # health it has not established is worse than none at all, which is the whole
  # reason the CVE watchdog was removed rather than left running (#3834). The
  # deciding half is therefore a plain module with cron arithmetic and a
  # measured jitter floor, and this derivation is what forces its suite to run.
  # Same reasoning as packages/ci-budget-policy: a suite nothing references is a
  # suite nobody notices failing.
  pkgs.runCommandLocal "scheduled-heartbeat" {
    __structuredAttrs = true;
    passthru.tests.unit =
      pkgs.runCommandLocal "scheduled-heartbeat-unit" {
        __structuredAttrs = true;
        nativeBuildInputs = [pkgs.nodejs];
      } ''
        # shell
        cp ${module} scheduled-heartbeat.mjs
        cp ${suite} scheduled-heartbeat.test.mjs
        node --test scheduled-heartbeat.test.mjs
        touch "$out"
      '';
    meta = {
      description = "Staleness decision for every scheduled workflow in this repository";
      license = pkgs.lib.licenses.mit;
    };
  } ''
    # shell
    mkdir -p "$out/lib"
    cp ${module} "$out/lib/scheduled-heartbeat.mjs"
  ''
