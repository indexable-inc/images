# What `ix.systemdHardening` + `StateDirectory=` actually does to ownership,
# measured rather than assumed (ENG-12400 / ENG-12406).
#
# The velocity module dropped its static `User=` for `DynamicUser = true` on
# the belief that "a nested `StateDirectory = "velocity/plugins"` leaf is left
# root-owned under PrivateUsers" (index#1649, 4ba5441c024). It is not. On
# systemd 261 the *leaf* of a nested StateDirectory is chowned to a static
# `User=` exactly as it is for a `DynamicUser=`; what stays root-owned is the
# intermediate directory systemd has to mkdir to reach that leaf, which
# systemd.exec(5) documents ("the innermost specified directories will be
# owned by the user and group specified in `User=` and `Group=`") and which
# happens identically under `DynamicUser=`. So `PrivateUsers=` is not the
# discriminator and a static `User=` needs no workaround.
#
# Each cell prints `ENG12400 <cell> <key> <value>` from inside the hardened
# unit, so every number asserted below is what the service itself sees through
# its own user and mount namespace, not what a root shell sees from outside.
#
# Two cells carry the properties that let `DynamicUser=` stop being
# load-bearing, so they are asserted rather than narrated:
#
#   stale-uid  a state tree that already exists owned by a uid the service no
#              longer runs as -- what a golden snapshot taken under a
#              differently-numbered `isSystemUser` allocation leaves behind.
#              systemd re-chowns it recursively on start for static users too,
#              and has since v242 (systemd#11842, systemd#12005).
#   migrated   a state tree a `DynamicUser=` created, inherited by a static
#              `User=`: the migration any service takes coming back off the
#              workaround.
{
  lib,
  pkgs,
  ix,
}: let
  # The uid a golden snapshot froze the `stale-uid` tree as. Outside the range
  # NixOS allocates from, so nothing in the test VM resolves it to a name.
  staleUid = 4999;

  # `stat -L` (follow) rather than lstat: under `DynamicUser=` systemd hands
  # the service a symlink into /var/lib/private, and the interesting inode is
  # the target.
  probe = name: paths: ''
    echo "ENG12400 ${name} euid $(id -u)"
    ${lib.concatMapStringsSep "\n" (path: ''
        if owner=$(stat -L -c '%u %g %a' ${lib.escapeShellArg path} 2>&1); then
          echo "ENG12400 ${name} owner ${path} $owner"
        else
          echo "ENG12400 ${name} owner ${path} unstattable: $owner"
        fi
      '')
      paths}
    leaf=${lib.escapeShellArg (lib.last paths)}
    if err=$(touch "$leaf/written-by-the-service" 2>&1); then
      echo "ENG12400 ${name} write ok"
    else
      echo "ENG12400 ${name} write fail $err"
    fi
  '';

  # A cell is one hardened unit plus the user it runs as; no `uid` means
  # `DynamicUser=`. `paths` is every level of the StateDirectory, outermost
  # first, and the last one is the leaf the write probe targets.
  mkCell = pkgs: {
    name,
    stateDirectory,
    paths,
    uid ? null,
    extraProbe ? "",
    after ? [],
  }: let
    user = "eng12400-${name}";
  in {
    users = lib.optionalAttrs (uid != null) {
      groups.${user}.gid = uid;
      users.${user} = {
        inherit uid;
        description = "ENG-12400 ${name} cell";
        group = user;
        isSystemUser = true;
      };
    };
    service = {
      description = "ENG-12400 ${name} cell";
      wantedBy = ["multi-user.target"];
      # The stale-uid cell reads a tree tmpfiles lays down; the others do not
      # care, and ordering them all the same way keeps the cells comparable.
      after = ["systemd-tmpfiles-setup.service"] ++ after;
      requires = after;
      serviceConfig =
        ix.systemdHardening
        // {
          ExecStart = lib.getExe (ix.writeBashApplication pkgs {
            name = "eng12400-${name}";
            runtimeInputs = [pkgs.coreutils];
            text = probe name paths + extraProbe;
          });
          RemainAfterExit = true;
          StateDirectory = stateDirectory;
          Type = "oneshot";
        }
        // (
          if uid == null
          then {DynamicUser = true;}
          else {
            Group = user;
            User = user;
          }
        );
    };
  };

  cellSpecs = {
    static-flat = {
      uid = 4001;
      stateDirectory = "eng12400-static-flat";
      paths = ["/var/lib/eng12400-static-flat"];
    };
    static-nested = {
      uid = 4002;
      stateDirectory = "eng12400-static-nested/leaf";
      paths = [
        "/var/lib/eng12400-static-nested"
        "/var/lib/eng12400-static-nested/leaf"
      ];
      # The directory systemd had to mkdir to reach the leaf is root-owned by
      # design, and `ProtectSystem = "strict"` bind-mounts only the leaf
      # writable. Naming it here is the point: this, not the leaf, is what a
      # nested StateDirectory costs, and it is what #1649 was really hitting
      # when it wrote into `${dataDir}` from preStart.
      extraProbe = ''
        if err=$(touch /var/lib/eng12400-static-nested/written-into-the-parent 2>&1); then
          echo "ENG12400 static-nested parentwrite ok"
        else
          echo "ENG12400 static-nested parentwrite fail $err"
        fi
      '';
    };
    dynamic-flat = {
      stateDirectory = "eng12400-dynamic-flat";
      paths = ["/var/lib/eng12400-dynamic-flat"];
    };
    dynamic-nested = {
      stateDirectory = "eng12400-dynamic-nested/leaf";
      paths = [
        "/var/lib/eng12400-dynamic-nested"
        "/var/lib/eng12400-dynamic-nested/leaf"
      ];
    };
    # systemd 261 reads a nobody-owned state tree as "already id-mapped"
    # (exec-invoke.c: `st.st_uid == UID_NOBODY` sets do_chown=false,
    # idmapped=true) and hands the static user an id-mapped mount rather than
    # re-chowning, so the inherited tree reads as the static user's own.
    migrated = {
      uid = 4004;
      stateDirectory = "eng12400-migrated";
      paths = ["/var/lib/eng12400-migrated"];
      after = ["eng12400-seed-migrated.service"];
      extraProbe = ''
        echo "ENG12400 migrated seeded $(stat -L -c '%u %g' /var/lib/eng12400-migrated/seeded)"
      '';
    };
    stale-uid = {
      uid = 4003;
      stateDirectory = "eng12400-stale-uid";
      paths = ["/var/lib/eng12400-stale-uid"];
      extraProbe = ''
        frozen=/var/lib/eng12400-stale-uid/plugins/frozen
        echo "ENG12400 stale-uid frozen $(stat -L -c '%u %g' "$frozen")"
        if err=$(ln -sf /dev/null "$frozen.link" 2>&1); then
          echo "ENG12400 stale-uid plugindirwrite ok"
        else
          echo "ENG12400 stale-uid plugindirwrite fail $err"
        fi
      '';
    };
  };

  cellNames = lib.attrNames cellSpecs;
in
  pkgs.testers.runNixOSTest {
    name = "hardened-state-directory";

    # The cells merge `ix.systemdHardening` straight out of the module args,
    # the same way every service module in this repo does; normally injected
    # by `evalImageConfig`'s specialArgs.
    node.specialArgs.ix = ix;

    # A module function, not a bare attrset: every probe binary the guest runs
    # is built from the guest's own `pkgs`, not the orchestrating host's.
    nodes.machine = {pkgs, ...}: let
      cells = lib.mapAttrs (name: args: mkCell pkgs (args // {inherit name;})) cellSpecs;
    in {
      users = lib.mkMerge (lib.mapAttrsToList (_: cell: cell.users) cells);

      systemd.services =
        lib.mapAttrs' (name: cell: lib.nameValuePair "eng12400-${name}" cell.service) cells
        // {
          # Seeds the `migrated` cell's StateDirectory the way a DynamicUser
          # would have left it, so the static-User cell that follows inherits
          # a real /var/lib/private tree rather than a fresh directory.
          eng12400-seed-migrated = {
            description = "ENG-12400 migrated cell seed (DynamicUser)";
            wantedBy = ["multi-user.target"];
            serviceConfig =
              ix.systemdHardening
              // {
                DynamicUser = true;
                ExecStart = lib.getExe (ix.writeBashApplication pkgs {
                  name = "eng12400-seed-migrated";
                  runtimeInputs = [pkgs.coreutils];
                  text = "touch /var/lib/eng12400-migrated/seeded";
                });
                RemainAfterExit = true;
                StateDirectory = "eng12400-migrated";
                Type = "oneshot";
              };
          };
        };

      # The state tree a golden snapshot would leave behind: already present,
      # already populated, owned by a uid nothing on this system resolves.
      systemd.tmpfiles.rules = [
        "d /var/lib/eng12400-stale-uid 0755 ${toString staleUid} ${toString staleUid} -"
        "d /var/lib/eng12400-stale-uid/plugins 0755 ${toString staleUid} ${toString staleUid} -"
        "f /var/lib/eng12400-stale-uid/plugins/frozen 0644 ${toString staleUid} ${toString staleUid} -"
      ];
    };

    testScript = ''
      machine.wait_for_unit("multi-user.target")
      for cell in ${builtins.toJSON cellNames}:
          machine.wait_for_unit(f"eng12400-{cell}.service")

      journal = machine.succeed("journalctl -o cat -u 'eng12400-*' --no-pager")
      print(journal)

      facts = {}
      for line in journal.splitlines():
          fields = line.split(maxsplit=3)
          if len(fields) == 4 and fields[0] == "ENG12400":
              facts[(fields[1], fields[2])] = fields[3]

      # An `owner` line repeats per path within a cell, so the flat `facts`
      # map cannot hold them; key those by (cell, path) instead.
      owners = {}
      for line in journal.splitlines():
          fields = line.split()
          if len(fields) == 7 and fields[0] == "ENG12400" and fields[2] == "owner":
              owners[(fields[1], fields[3])] = (int(fields[4]), int(fields[5]))

      # A missing line and a wrong line must not read the same: assert every
      # cell reported before asserting what it reported, or a probe that never
      # ran passes the sweep below vacuously.
      for cell in ${builtins.toJSON cellNames}:
          assert (cell, "write") in facts, f"{cell} never reported a write result: {facts}"

      # Every cell can write its own leaf. This is the whole claim: a static
      # `User=` under `PrivateUsers=` is no worse off than a `DynamicUser=`.
      for cell in ${builtins.toJSON cellNames}:
          result = facts[(cell, "write")]
          assert result == "ok", f"{cell} could not write its StateDirectory: {result}"

      # Static users own their StateDirectory, flat and nested alike. If a
      # systemd change regresses this to root, these are the failing lines.
      assert owners[("static-flat", "/var/lib/eng12400-static-flat")] == (4001, 4001), owners
      assert owners[("static-nested", "/var/lib/eng12400-static-nested/leaf")] == (4002, 4002), owners

      # The intermediate directory of a nested path is root-owned and
      # unwritable, by design and identically under `DynamicUser=`. Asserted
      # so a nested StateDirectory is never again read as a leaf-ownership bug.
      assert owners[("static-nested", "/var/lib/eng12400-static-nested")] == (0, 0), owners
      assert owners[("dynamic-nested", "/var/lib/eng12400-dynamic-nested")] == (0, 0), owners
      parentwrite = facts[("static-nested", "parentwrite")]
      assert parentwrite.startswith("fail"), parentwrite

      # A tree left owned by a uid the service no longer is: systemd re-chowns
      # it recursively on start, so the service owns the frozen file too, not
      # just the top level.
      assert owners[("stale-uid", "/var/lib/eng12400-stale-uid")] == (4003, 4003), owners
      assert facts[("stale-uid", "frozen")] == "4003 4003", facts[("stale-uid", "frozen")]
      assert facts[("stale-uid", "plugindirwrite")] == "ok", facts[("stale-uid", "plugindirwrite")]

      # Coming back off the DynamicUser workaround: the static user reads the
      # tree its dynamic predecessor left as its own. If systemd ever stops
      # id-mapping an inherited nobody-owned state tree, this line catches it
      # before a service crash-loops.
      assert facts[("migrated", "seeded")] == "4004 4004", facts[("migrated", "seeded")]

      # Ownership as the rest of the system sees it, for the record.
      print(machine.succeed(
          "stat -c '%u %g %a %n' /var/lib/eng12400-* /var/lib/eng12400-*/leaf"
          " /var/lib/eng12400-stale-uid/plugins/frozen"
      ))
      print(machine.succeed("systemd-analyze --version | head -1"))
    '';
  }
