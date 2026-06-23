{
  lib,
  nodes,
  pkgs,
  ...
}:
let
  share = {
    mountPoint = "/mnt/share";
    name = "share";
  };
  serverHost = nodes.file-server.config.ix.networking.eastWest.hostName;
in
{
  # The real dependency is the ix guest kernel: `linux-ix` must build in CIFS
  # support or ship `cifs.ko` in the module tree injected at
  # `/ix-guest/lib/modules`. This image only supplies `mount.cifs`.
  # Linux's cifs translates `flock()` to a byte-range lock on the whole file
  # (kernel >= 5.4), so both `flock` and `fcntl` byte-range locks reach the
  # server's lock manager and coordinate across mounts.
  fileSystems.${share.mountPoint} = {
    device = "//${serverHost}/${share.name}";
    fsType = "cifs";
    options = [
      # Pin the newest SMB dialect Samba serves in this example so client and
      # server use the same locking and lease behavior.
      "vers=3.1.1"
      # Match the server's guest-writable demo share; production mounts should
      # use a credentials file instead.
      "guest"
      # Present remote files as root-owned inside the VM, matching the operator
      # model for these examples.
      "uid=0"
      # Keep group ownership root-side too, so new files line up with the mode
      # bits below.
      "gid=0"
      # Files created over the share are writable by owner and group only.
      "file_mode=0660"
      # Directories need execute bits so owner and group can traverse them.
      "dir_mode=0770"
      # `nobrl` disables byte-range locking. Leaving it absent keeps BRL
      # enabled, which is the behavior this example demonstrates.
      # Cache attributes for 1 second so metadata from `client-0` becomes
      # visible to `client-1` promptly without turning off the cache entirely.
      "actimeo=1"
      # Tell systemd this mount depends on networking, so it is ordered like a
      # remote filesystem.
      "_netdev"
      # Allow boot to continue if the server is still coming up; the health
      # check below reports the missing mount afterward.
      "nofail"
      # Pull in network-online.target before attempting the CIFS mount.
      "x-systemd.requires=network-online.target"
      # Place the mount attempt after network-online.target in systemd's order.
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
      share.mountPoint
    ];
  };
}
