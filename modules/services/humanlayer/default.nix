# HumanLayer (riptide) remote daemon.
#
# Runs `humanlayer daemon launch` as a long-lived systemd service so an ix VM
# can act as a remote HumanLayer host: the daemon dials out to the HumanLayer
# cloud and drives coding sessions on this guest. It binds no inbound socket
# (outbound-only), so there is no port claim.
#
# Auth: the daemon needs a launch token. Mint one on an authenticated host with
# `humanlayer api auth daemon launch-token create` (or copy the launch command
# from app.humanlayer.com) and place the token at {option}`launchTokenFile`
# through your secret mechanism. The token is read at service start and is never
# baked into the image. The daemon then exchanges the launch token for its own
# daemon credentials. Mirrors the runtime-secret idiom in
# modules/services/ci-runner (a `tokenFile` runtime path string).
{
  config,
  lib,
  pkgs,
  ...
}: let
  inherit
    (lib)
    mkEnableOption
    mkIf
    mkOption
    mkPackageOption
    types
    ;
  cfg = config.services.humanlayer;

  # Environment selector. The CLI defaults to prod when no flag is passed; the
  # other environments are opt-in for beta/dev/local backends.
  envArg =
    if cfg.environment == null
    then ""
    else
      {
        beta = " --beta";
        dev = " --dev";
        local = " --local";
      }
      .${
        cfg.environment
      };
in {
  options.services.humanlayer = {
    enable = mkEnableOption "the HumanLayer (riptide) remote daemon";

    package = mkPackageOption pkgs "humanlayer" {};

    launchTokenFile = mkOption {
      # A runtime path string, not `types.path`: the file is provided at boot by
      # a secret mechanism outside the image, not a build input.
      type = types.str;
      example = "/run/secrets/humanlayer/launch-token";
      description = ''
        Path to a file holding a HumanLayer launch token, read when the daemon
        starts. Mint one on an authenticated host with
        `humanlayer api auth daemon launch-token create` and place it here. The
        token is never baked into the image.
      '';
    };

    environment = mkOption {
      type = types.nullOr (
        types.enum [
          "beta"
          "dev"
          "local"
        ]
      );
      default = null;
      description = ''
        HumanLayer backend environment. `null` uses the CLI default (prod). Set
        to `beta`, `dev`, or `local` to target a non-production backend.
      '';
    };

    stateDir = mkOption {
      type = types.str;
      default = "/var/lib/humanlayer";
      description = ''
        HOME for the daemon. The riptide daemon keeps its host id, logs, plugins,
        and artifacts under `$HOME/.humanlayer`.
      '';
    };
  };

  config = mkIf cfg.enable {
    systemd.services.humanlayer-daemon = {
      description = "HumanLayer (riptide) remote daemon";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];
      path =
        [
          cfg.package
          pkgs.coreutils
        ]
        ++ config.environment.systemPackages;
      environment.HOME = cfg.stateDir;

      # The CLI only accepts the launch token as an argument, so read it from the
      # secret file at start and exec the daemon. Argv exposure is not a boundary
      # here: a root process inside the guest is fully capable anyway (see the
      # image-conventions skill), and the secret never leaves the VM.
      script = ''
        token="$(cat ${lib.escapeShellArg cfg.launchTokenFile})"
        exec ${lib.getExe cfg.package} daemon launch --launch-token "$token"${envArg}
      '';

      serviceConfig = {
        Restart = "on-failure";
        RestartSec = 5;
        StateDirectory = "humanlayer";
        StateDirectoryMode = "0700";
      };
    };
  };
}
