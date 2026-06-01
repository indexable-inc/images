# OCI layer: builds the final OCI archive from the NixOS system closure.
#
# The closure is split into ~67 OCI layers (`streamLayeredImage`) so the
# registry deduplicates shared store paths across images. A `systemRoot`
# layer adds FHS entries (/bin, /etc, /usr, ...) needed at boot.
#
# nixpkgs' layer planner is reused, but the final archive is streamed as OCI
# directly so large images do not pay for a Docker archive transcode pass.
{
  config,
  pkgs,
  lib,
  ...
}:
{
  options.ix = {
    image = {
      name = lib.mkOption {
        type = lib.types.str;
        description = "Image name (the OCI repository).";
      };
      tag = lib.mkOption {
        type = lib.types.str;
        default = "latest";
        description = "Image tag.";
      };
    };
    build.ociImage = lib.mkOption {
      type = lib.types.package;
      internal = true;
    };
    build.ociEfficiency = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Analyze generated OCI layers and fail builds when wasted payload crosses the configured limits.";
      };
      minEfficiency = lib.mkOption {
        type = lib.types.number;
        default = 0.95;
        description = "Minimum layer efficiency score accepted by the build. The score is required payload bytes divided by discovered payload bytes.";
      };
      maxWastedBytes = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 20 * 1024 * 1024;
        description = "Maximum wasted layer payload bytes accepted before the build fails.";
      };
      maxWastedPercent = lib.mkOption {
        type = lib.types.number;
        default = 0.20;
        description = "Maximum wasted payload ratio accepted before the build fails.";
      };
      reportTopPaths = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 10;
        description = "Number of repeated or removed paths to print when wasted payload is present.";
      };
    };
  };

  config.ix = {
    profiles.base.enable = lib.mkDefault true;

    build.ociImage =
      let
        inherit (config.system.build) toplevel;

        # FHS layout pointing into the NixOS toplevel. Keep activation-owned
        # paths writable: NixOS first boot populates /etc and creates /bin/sh
        # and /usr/bin/env, so those cannot be symlinks into the immutable store.
        systemRoot = pkgs.runCommand "system-root" { } ''
          mkdir -p $out
          ln -s ${toplevel}/init $out/init
          mkdir -p $out/etc
          mkdir -p $out/bin
          ln -s ${toplevel}/sw/sbin $out/sbin
          ln -s ${toplevel}/sw/lib $out/lib
          mkdir -p $out/usr/bin
          ln -s ${toplevel}/sw/lib $out/usr/lib
          ln -s ${toplevel}/sw/sbin $out/usr/sbin
          mkdir -p $out/tmp $out/var $out/run $out/proc $out/sys $out/dev $out/root
        '';

        stream = pkgs.dockerTools.streamLayeredImage {
          inherit (config.ix.image) name tag;
          # Below the 127-layer registry limit with headroom for systemRoot
          # plus a few user layers.
          maxLayers = 67;
          contents = [ systemRoot ];
          config.Entrypoint = [ "${toplevel}/init" ];
        };

        efficiency = config.ix.build.ociEfficiency;
        efficiencyArgs =
          if efficiency.enable then
            [
              "--min-efficiency"
              (toString efficiency.minEfficiency)
              "--max-wasted-bytes"
              (toString efficiency.maxWastedBytes)
              "--max-wasted-percent"
              (toString efficiency.maxWastedPercent)
              "--efficiency-top-paths"
              (toString efficiency.reportTopPaths)
            ]
          else
            [ "--skip-efficiency-check" ];
      in
      pkgs.runCommand "${config.ix.image.name}-oci.tar"
        {
          nativeBuildInputs = [
            pkgs.coreutils
            pkgs.gnutar
            pkgs.oci-image-builder
          ];
        }
        ''
          oci-image-builder ${lib.escapeShellArgs (efficiencyArgs ++ [ "${stream.passthru.conf}" ])} "$out"
        '';
  };
}
