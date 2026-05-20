{
  config,
  lib,
  nodes,
  pkgs,
  ...
}:
let
  cfg = config.services.file-share-client;
  serverHost = nodes.file-server.config.ix.networking.eastWest.hostName;
in
{
  options.services.file-share-client = {
    mountPoint = lib.mkOption {
      type = lib.types.str;
      default = "/mnt/share";
      description = "Local path where the CIFS share is mounted.";
    };

    shareName = lib.mkOption {
      type = lib.types.str;
      default = "share";
      description = "Samba share name exported by the file-server node.";
    };
  };

  config = {
    # `cifs.ko` is loaded from the host `linux-ix` kernel; we don't set
    # `boot.supportedFilesystems` because a `boot.isContainer = true`
    # image can't `modprobe` host modules itself. The mount unit will
    # surface a clear error if cifs is unavailable on the host kernel.
    # Linux's cifs translates `flock()` to a byte-range lock on the
    # whole file (kernel >= 5.4), so both `flock` and `fcntl` byte-range
    # locks reach the server's lock manager and coordinate across mounts.

    fileSystems.${cfg.mountPoint} = {
      device = "//${serverHost}/${cfg.shareName}";
      fsType = "cifs";
      options = [
        "vers=3.1.1"
        "guest"
        "uid=0"
        "gid=0"
        "file_mode=0660"
        "dir_mode=0770"
        # We deliberately do NOT pass `nobrl`: it is a pure flag that
        # disables byte-range locking, the very thing this example
        # exists to demonstrate. The default is BRL enabled. Don't add
        # `nobrl` here without rewriting the README.
        # Short attribute cache keeps cross-client metadata reads fresh
        # without disabling the cache entirely.
        "actimeo=1"
        "_netdev"
        "nofail"
        "x-systemd.requires=network-online.target"
        "x-systemd.after=network-online.target"
      ];
      noCheck = true;
    };

    environment.systemPackages = [
      pkgs.cifs-utils
      # `flock(1)` lives here; pre-installed so `ix shell client-0 -- flock`
      # works without reaching for `nix run nixpkgs#util-linux`.
      pkgs.util-linux
    ];

    ix.healthChecks.cifs-mount = {
      description = "CIFS share is mounted";
      command = [
        (lib.getExe' pkgs.util-linux "findmnt")
        "--type"
        "cifs"
        cfg.mountPoint
      ];
    };
  };
}
