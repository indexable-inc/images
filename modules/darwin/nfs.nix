# Declarative NFS automounts for nix-darwin, via the autofs machinery macOS
# ships (automountd). Each entry renders one line of a direct map
# (/etc/auto_index), the include line is added to /etc/auto_master
# idempotently at activation, and `automount -vc` reloads the maps — no
# daemons, no kexts, and mounts appear on first access like any autofs mount.
#
# macOS never got FSKit-based NFS (FSKit ships with only msdos in practice)
# and nix-darwin has no mount module, so autofs is the supported, native path
# for "declared network mounts" on Darwin.
{
  config,
  lib,
  pkgs,
  ...
}: let
  inherit
    (lib)
    mkOption
    mkIf
    types
    ;

  cfg = config.services.nfs;

  mountType = types.submodule ({name, ...}: {
    options = {
      server = mkOption {
        type = types.str;
        example = "nas.local";
        description = "NFS server host.";
      };

      remotePath = mkOption {
        type = types.str;
        example = "/export/media";
        description = "Exported path on the server.";
      };

      options = mkOption {
        type = types.listOf types.str;
        default = [
          "resvport"
          "nfc"
        ];
        example = [
          "resvport"
          "nfsvers=4"
          "soft"
        ];
        description = ''
          Mount options for the map entry. `resvport` is required by most
          Linux NFS servers' default export policy; `nfc` normalizes unicode
          filenames. Add `nfsvers=4`/`soft`/`intr` etc. as the server needs.
        '';
      };

      _mountpoint = mkOption {
        type = types.str;
        internal = true;
        default =
          if lib.hasPrefix "/" name
          then name
          else "/${name}";
        description = "Absolute local mount point (the attribute name).";
      };
    };
  });

  mounts = lib.attrValues cfg.automounts;

  mapLine = mount: "${mount._mountpoint} -${lib.concatStringsSep "," mount.options} ${mount.server}:${mount.remotePath}";

  mapFile = "auto_index";
in {
  options.services.nfs.automounts = mkOption {
    type = types.attrsOf mountType;
    default = {};
    example = lib.literalExpression ''
      {
        "/Volumes/media" = {
          server = "nas.local";
          remotePath = "/export/media";
          options = [ "resvport" "nfsvers=4" ];
        };
      }
    '';
    description = ''
      Declarative NFS automounts (macOS autofs). Attribute names are local
      mount points; each renders a direct-map entry in /etc/${mapFile}, and
      activation reloads automountd. Mounts attach lazily on first access
      and detach when idle, like any autofs mount.
    '';
  };

  config = mkIf (cfg.automounts != {}) {
    environment.etc.${mapFile}.text =
      lib.concatMapStrings (mount: mapLine mount + "\n") mounts;

    # /etc/auto_master is Apple-owned and may gain entries across OS updates,
    # so it is amended in place (idempotently) rather than replaced through
    # environment.etc.
    system.activationScripts.postActivation.text = lib.mkAfter ''
      if ! /usr/bin/grep -q '^/-[[:space:]]\{1,\}${mapFile}' /etc/auto_master 2>/dev/null; then
        printf '/-\t${mapFile}\t-nosuid\n' >> /etc/auto_master
        echo "added ${mapFile} direct map to /etc/auto_master"
      fi
      /usr/sbin/automount -vc
    '';
  };
}
