/**
  SMB share module builders for the dev-fleet identity volume.

  Generalized from `examples/multi-client-file-sharing`: one node runs
  userspace `smbd` (ix guests share the host `linux-ix` kernel, so the
  server cannot be in-kernel `ksmbd`) and exports a single share; every
  other node mounts it over CIFS with `cifs.ko` from the host kernel. The
  locking knobs (`strict locking`, `posix locking`, `strict sync`) are what
  keep concurrent readers/writers honest, which is exactly what a shared
  `~/.claude/.credentials.json` needs during a token refresh.

  Returns `{ serverModule, clientModule }`. `mkDev` calls these with the
  share name, directory, and the elected server node; it does not configure
  Samba itself.

  Security note: `guestOk` defaults to `true` so the example fleet comes up
  with `ix up` and no secrets plumbing, the same tradeoff
  `examples/multi-client-file-sharing` documents. The share is only reachable
  on the fleet's private east-west group (mkDev joins one), never publicly.
  A production identity volume should set `guestOk = false`, add a Samba user
  with `smbpasswd`, and pass `credentials=` to the client mount through a
  systemd `LoadCredential` (RFC 0007, and the shape `python-daily-scraper`
  uses for AWS keys).
*/
{ lib }:
let
  sambaPort = 445;
in
{
  /**
    smbd configuration for the elected server node.

    Arguments:
    - `shareName`: the SMB share name clients mount (`//server/<shareName>`).
    - `shareDir`: on-disk path the share exports.
    - `guestOk`: allow unauthenticated access (demo default; see file header).
    - `subdirs`: directories to pre-create under `shareDir` so client-side
      bind mounts (`~/.claude`, `~/.n`, `/ix`) have an existing target.
  */
  serverModule =
    {
      shareName,
      shareDir,
      guestOk ? true,
      subdirs ? [ ],
    }:
    _: {
      services.samba = {
        enable = true;
        # `ix.networking.expose.samba` below claims the port and opens the
        # in-guest firewall, so the listener policy stays in one place.
        openFirewall = false;

        settings = {
          global = {
            "workgroup" = "WORKGROUP";
            "server string" = "ix dev fleet identity volume";
            "security" = "user";
            "map to guest" = "Bad User";
            "guest account" = "nobody";

            # SMB 3.1.1 on both ends: the newest dialect Samba serves here,
            # with the byte-range lock and lease behavior the clients rely on.
            "server min protocol" = "SMB3_11";
            "client min protocol" = "SMB3_11";

            # Mediate every byte-range lock in `smbd` rather than pushing it
            # down to the host kernel's local-fs locks, so two CIFS clients
            # coordinate `flock()`/`fcntl` locks against each other.
            "locking" = "yes";
            "strict locking" = "yes";
            "posix locking" = "yes";
            "oplocks" = "yes";
            "kernel oplocks" = "no";

            # Cross-client visibility over throughput: a credential refresh on
            # one node is readable from another without waiting for a cache to
            # expire on its own.
            "strict sync" = "yes";
          };

          ${shareName} = {
            "path" = shareDir;
            "browseable" = "yes";
            "read only" = "no";
            "writable" = "yes";
            "guest ok" = if guestOk then "yes" else "no";
            "force user" = "nobody";
            "force group" = "nogroup";
            "create mask" = "0660";
            "directory mask" = "0770";
          };
        };
      };

      systemd.tmpfiles.rules = [
        "d ${shareDir} 0770 nobody nogroup -"
      ]
      ++ map (sub: "d ${shareDir}/${sub} 0770 nobody nogroup -") subdirs;

      # One declaration registers the claim and opens the firewall for SMB.
      ix.networking.expose.samba = {
        port = sambaPort;
        description = "Samba SMB3 identity volume for the dev fleet";
      };

      ix.healthChecks.smbd = {
        description = "smbd is active";
        unit = "samba-smbd";
      };
    };

  /**
    CIFS mount for a workload node.

    Arguments:
    - `serverNode`: fleet node name running `smbd` (resolved to its east-west
      hostname through `nodes`, so renaming the node moves the mount with it).
    - `shareName`: the share to mount.
    - `mountPoint`: where to mount it in the guest.
    - `guest`: mount without credentials (matches `guestOk` on the server).
  */
  clientModule =
    {
      serverNode,
      shareName,
      mountPoint,
      guest ? true,
    }:
    {
      lib,
      nodes,
      pkgs,
      ...
    }:
    let
      serverHost = nodes.${serverNode}.config.ix.networking.eastWest.hostName;
    in
    {
      fileSystems.${mountPoint} = {
        device = "//${serverHost}/${shareName}";
        fsType = "cifs";
        options = [
          # Pin the newest SMB dialect the server serves so both ends agree on
          # locking and lease behavior.
          "vers=3.1.1"
        ]
        ++ lib.optional guest "guest"
        ++ [
          # Present remote files as root-owned inside the VM, matching the
          # operator model: the dev VM runs its agent as root.
          "uid=0"
          "gid=0"
          "file_mode=0660"
          "dir_mode=0770"
          # Cache attributes for 1s so a write from one node becomes visible on
          # another promptly without disabling the cache entirely.
          "actimeo=1"
          # Order the mount like a remote filesystem.
          "_netdev"
          # Boot continues if the server is still coming up; the health check
          # reports the missing mount afterward.
          "nofail"
          "x-systemd.requires=network-online.target"
          "x-systemd.after=network-online.target"
        ];
        noCheck = true;
      };

      environment.systemPackages = [
        pkgs.cifs-utils
        pkgs.util-linux
      ];

      ix.healthChecks.identity-volume-mount = {
        description = "identity volume CIFS share is mounted";
        command = [
          (lib.getExe' pkgs.util-linux "findmnt")
          "--type"
          "cifs"
          mountPoint
        ];
      };
    };
}
