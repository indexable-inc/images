# NixOS module for `jj fs mount`: serve jj revisions as read-only filesystems.
#
# NixOS only, and that is a real limit rather than an oversight. The macOS
# deployment surface would be a launchd agent under nix-darwin, which shares no
# structure with a systemd unit, so it is a separate module that does not exist
# yet. On macOS today the command is run by hand; see docs/vfs.md.
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.jj-vfs;

  # Default NFS port. Above 1024 on purpose: a reserved port is precisely what
  # would force the client to run as root, and nothing in NFSv3 requires one.
  # 20049 rather than the standard 2049 so a jj mount can never be mistaken for,
  # or collide with, a real NFS server on the same host.
  defaultNfsPort = 20049;

  mountOpts = {name, ...}: {
    options = {
      repository = lib.mkOption {
        type = lib.types.path;
        description = ''
          Path to the jj repository whose revision is served. The mount reads
          this repository; it never writes to it.
        '';
        example = "/srv/src/myproject";
      };

      revision = lib.mkOption {
        type = lib.types.str;
        default = "@";
        description = ''
          Revset selecting the revision to serve. Must resolve to exactly one
          revision. The tree is read once at mount time and never changes while
          the mount is up.
        '';
        example = "main";
      };

      mountPoint = lib.mkOption {
        type = lib.types.path;
        default = "/mnt/${name}";
        defaultText = lib.literalExpression ''"/mnt/''${name}"'';
        description = ''
          Where to mount. Created if absent and required to be empty, because
          mounting over a populated directory hides its contents until the
          unmount and reads as data loss to whoever it happens to.
        '';
      };

      transport = lib.mkOption {
        type = lib.types.enum ["fuse" "nfs"];
        default = "fuse";
        description = ''
          Which kernel interface to serve over.

          `fuse` is the default here because this is a NixOS module and FUSE on
          Linux is one kernel hop, needing no privileges and no helper. `nfs`
          works but must be mounted by root on Linux, unlike on macOS where it
          is the only unprivileged option and therefore the default there.
        '';
      };

      nfsPort = lib.mkOption {
        type = lib.types.port;
        default = defaultNfsPort;
        description = ''
          TCP port for the loopback NFSv3 server, used only when
          {option}`transport` is `nfs`. Bound on 127.0.0.1 only: an NFSv3 server
          has no authentication, so binding anywhere else would publish the
          revision to the network. Must stay above 1024, or mounting it would
          need a reserved port and therefore root.
        '';
      };

      readOnly = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Whether the mount is read-only. Only `true` is supported today; the
          option exists so that gaining a write path is a visible change in
          configuration rather than a silent change in behavior.
        '';
      };

      contentCacheBytes = lib.mkOption {
        type = lib.types.ints.positive;
        default = 256 * 1024 * 1024;
        description = ''
          Byte budget for cached file contents. A conflicted file and a symlink
          must be built before they can be sized, so a tree with many of either
          benefits from a larger budget. Ordinary files are sized from the store
          without being read and do not depend on this.
        '';
      };

      user = lib.mkOption {
        type = lib.types.str;
        default = "root";
        description = ''
          User the server runs as, and the owner reported for every entry in the
          mount. Must be able to read {option}`repository`.
        '';
      };

      group = lib.mkOption {
        type = lib.types.str;
        default = "root";
        description = "Group the server runs as, and the group reported for every entry.";
      };

      flakeRoot = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = ''
          Where a `flake.nix` inside the mount is expected, if any. Setting it
          buys an eval-time check of a constraint that is otherwise discovered at
          use time: Nix's upward search for `flake.nix` refuses to cross a
          filesystem boundary and fails with `error: unable to find a flake
          before encountering filesystem boundary`, so a flake root above the
          mount point simply does not work.
        '';
        example = "/mnt/main";
      };

      startAtBoot = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Whether the mount comes up with the system. Set false for a mount whose
          repository is not present at boot, or one that should be started on
          demand; the unit is still defined and `systemctl start` works.
        '';
      };

      journalDirectory = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = ''
          Where the change journal is written, once there is one. Nothing is
          written today, because a read-only mount has no changes to record.

          It must not be inside {option}`mountPoint`, and the module refuses that
          at eval time. Over NFS neither the macOS nor the Linux client sends
          COMMIT on fsync, so a journal written through the mount it is
          recording can lose its tail while looking complete, which is worse than
          having no journal at all.
        '';
      };
    };
  };

  # A path is "under" another if it is that path or is prefixed by it plus a
  # separator. Comparing with a trailing separator on both sides is what stops
  # /mnt/foobar from counting as under /mnt/foo.
  isUnder = parent: child:
    lib.hasPrefix "${toString parent}/" "${toString child}/";

  unitName = name: "jj-vfs-${name}";

  mountScript = name: mount: let
    args =
      [
        "fs"
        "mount"
        "-r"
        mount.revision
        "--transport"
        mount.transport
        "--content-cache-bytes"
        (toString mount.contentCacheBytes)
      ]
      ++ lib.optionals (mount.transport == "nfs") [
        "--nfs-port"
        (toString mount.nfsPort)
      ]
      ++ [mount.mountPoint];
  in
    pkgs.writeShellScript "${unitName name}-start" ''
      set -euo pipefail
      # `jj fs mount` requires an empty directory and says so, but the unit has
      # to create it first or a fresh host fails on its very first start.
      mkdir -p ${lib.escapeShellArg mount.mountPoint}
      cd ${lib.escapeShellArg mount.repository}
      exec ${lib.getExe cfg.package} ${lib.escapeShellArgs args}
    '';

  # Runs on every exit, clean or not, rather than only on the failure path, so
  # the two paths are identical and there is one teardown to get right.
  #
  # Teardown deliberately does not read the mount. Under NFS a mount whose server
  # has gone away blocks every syscall against it until the client times out, so
  # a stop path that stats the mountpoint to decide what to do can hang for the
  # length of that timeout. umount is safe because it acts on the mount table
  # rather than on the filesystem's contents.
  stopScript = name: mount:
    pkgs.writeShellScript "${unitName name}-stop" ''
      set -uo pipefail
      # jj unmounts itself on SIGTERM, so the usual outcome here is "not
      # currently mounted" and that is success. This is the belt to that braces:
      # after SIGKILL nothing ran on the way out and the mount is still there.
      #
      # Stderr is left going to the journal rather than discarded, because the
      # difference between "already unmounted" and "target is busy" is the whole
      # diagnostic value of this script.
      umount ${lib.escapeShellArg mount.mountPoint} \
        || umount -f ${lib.escapeShellArg mount.mountPoint} \
        || true
    '';

  anyNfs = lib.any (m: m.transport == "nfs") (lib.attrValues cfg.mounts);
in {
  options.services.jj-vfs = {
    package = lib.mkPackageOption pkgs "jujutsu" {};

    mounts = lib.mkOption {
      type = lib.types.attrsOf (lib.types.submodule mountOpts);
      default = {};
      description = ''
        Revisions to serve as filesystems, one systemd service each. One process
        serves one revision, so several mounts means several services.
      '';
      example = lib.literalExpression ''
        {
          main = {
            repository = "/srv/src/myproject";
            revision = "main";
          };
        }
      '';
    };
  };

  config = lib.mkIf (cfg.mounts != {}) {
    assertions =
      lib.concatLists (lib.mapAttrsToList (name: mount: [
          {
            assertion = mount.readOnly;
            message = ''
              services.jj-vfs.mounts.${name}.readOnly is false, but this version of
              `jj fs mount` has no write path. Leave it true.
            '';
          }
          {
            assertion = mount.flakeRoot == null || isUnder mount.mountPoint mount.flakeRoot;
            message = ''
              services.jj-vfs.mounts.${name}.flakeRoot (${toString mount.flakeRoot}) is not
              at or below mountPoint (${toString mount.mountPoint}). Nix's upward search for
              flake.nix refuses to cross a filesystem boundary, so a flake root outside the
              mount fails with "unable to find a flake before encountering filesystem
              boundary".
            '';
          }
          {
            assertion =
              mount.journalDirectory == null
              || !(isUnder mount.mountPoint mount.journalDirectory);
            message = ''
              services.jj-vfs.mounts.${name}.journalDirectory
              (${toString mount.journalDirectory}) is inside mountPoint
              (${toString mount.mountPoint}). The journal must go to real storage: over NFS
              neither client sends COMMIT on fsync, so a journal written through the mount
              it records can lose its tail while looking complete.
            '';
          }
          {
            assertion = mount.transport != "nfs" || mount.nfsPort > 1024;
            message = ''
              services.jj-vfs.mounts.${name}.nfsPort is ${toString mount.nfsPort}, a reserved
              port. Mounting from one requires root on the client for no benefit; pick a
              port above 1024.
            '';
          }
        ])
        cfg.mounts);

    # mount.nfs lives in nfs-utils and the NFSv3 client is a kernel module that
    # is not autoloaded. Only pulled in when a mount actually asks for NFS.
    boot.supportedFilesystems.nfs = lib.mkIf anyNfs true;

    systemd.services =
      lib.mapAttrs' (name: mount:
        lib.nameValuePair (unitName name) {
          description = "jj filesystem mount of ${mount.revision} at ${mount.mountPoint}";
          wantedBy = lib.optional mount.startAtBoot "multi-user.target";
          after = ["local-fs.target"];
          path = [pkgs.util-linux] ++ lib.optional (mount.transport == "nfs") pkgs.nfs-utils;

          serviceConfig = {
            Type = "exec";
            ExecStart = mountScript name mount;
            ExecStopPost = stopScript name mount;
            User = mount.user;
            Group = mount.group;

            # Deliberately not restarted, and this is the load-bearing decision
            # in the unit. Do not change it to on-failure from general systemd
            # habit; a restart here does not produce a fresh mount.
            #
            # A restart gives a new server process behind a mount the kernel
            # still holds. Over NFS the client has cached file handles carrying
            # the old server's generation number, so every one of them goes
            # stale against the new server: the mount keeps answering, and
            # answers wrongly. That is the same failure class as under-reporting
            # st_size, where Nix stores content under the hash of bytes that
            # never existed. A hung mount is loud, contained and fixable by a
            # human who can see it; a mount serving stale content quietly is
            # none of those. Prefer the loud failure.
            #
            # FUSE and NFS differ in how the death shows up, ENOTCONN against a
            # hang, and this is deliberately the same for both rather than tuned
            # per transport: one teardown path is one path to get right.
            #
            # Making this restartable needs stable, content-derived file handle
            # generations first, not a policy change here.
            Restart = "no";

            # jj exits 1 when a signal unmounts it, which is its convention for
            # any interrupted command rather than a fault, so systemd should not
            # record the ordinary stop as a failure.
            SuccessExitStatus = [1];
          };
        })
      cfg.mounts;
  };
}
