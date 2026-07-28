/**
The whoami job, declared next to the machines that run it and submitted at
boot.

The spec is plain Nix rendered to nomad's API JSON by one renderer
(pkgs.formats.json); `nomad job run -json` reads that format directly, so
there is no HCL to hand-assemble. The task's artifact is the store path of
whoami.nix, which every client pins into its closure. Count matches the
client VMs and the static port keeps allocations from co-locating, so
the scheduler spreads one allocation per client.
*/
{
  config,
  ix,
  lib,
  nodes,
  pkgs,
  ...
}: let
  ports = import ./ports.nix;
  whoami = import ./whoami.nix {inherit ix pkgs;};

  clientCount =
    builtins.length
    (builtins.filter (lib.hasPrefix "nomad-client") (builtins.attrNames nodes));

  jobSpec = (pkgs.formats.json {}).generate "whoami.nomad.json" {
    Job = {
      ID = "whoami";
      Name = "whoami";
      Type = "service";
      Datacenters = [config.services.nomad.settings.datacenter];
      TaskGroups = [
        {
          Name = "web";
          Count = clientCount;
          Networks = [
            {
              ReservedPorts = [
                {
                  Label = "http";
                  Value = ports.app;
                }
              ];
            }
          ];
          Tasks = [
            {
              Name = "whoami";
              Driver = "raw_exec";
              Config.command = lib.getExe whoami;
            }
          ];
        }
      ];
    };
  };

  nomad = lib.getExe config.services.nomad.package;
in {
  # Boot-time `nomad job run`, the moral equivalent of the k3s auto-deploy
  # directory (nomad has no built-in one). The API takes a moment after the
  # unit starts, so the submit polls with a bounded budget and fails the unit
  # loudly if the deadline passes.
  systemd.services.nomad-job-whoami = {
    description = "submit the whoami job to nomad";
    wantedBy = ["multi-user.target"];
    requires = ["nomad.service"];
    after = ["nomad.service"];
    path = [pkgs.coreutils];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
    };
    script = ''
      for _ in $(seq 60); do
        if ${nomad} job run -json ${jobSpec}; then
          exit 0
        fi
        sleep 2
      done
      echo "nomad-job-whoami: nomad did not accept the job within 120s" >&2
      exit 1
    '';
  };

  # `job inspect` exits non-zero until the job is registered; each client's
  # own `whoami-http` probe (client.nix) then covers "my allocation runs and
  # answers", so together the example's checks gate on one alloc per client.
  ix.healthChecks.whoami-registered = {
    description = "whoami job registered with the scheduler";
    command = [
      nomad
      "job"
      "inspect"
      "whoami"
    ];
  };
}
