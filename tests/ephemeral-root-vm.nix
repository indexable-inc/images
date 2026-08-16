# The wipe fires, the whitelist survives it, and the evidence is recoverable.
#
# `modules/system/ephemeral-root` deletes the root filesystem on every boot.
# Every other check in this repo is eval-time, and eval-time is exactly the
# wrong tier for this one: a module that produces a correct-looking
# `fileSystems` set and a correct-looking initrd unit can still boot to an
# emergency shell, or boot cleanly and quietly keep nothing. Only a machine
# that actually reboots can tell those apart.
#
# WHAT THIS TEST HOLDS, in order of how expensive the failure would be:
#
#   1. The machine comes back at all. A rollback that runs at the wrong point
#      in the initrd takes the root out from under `sysroot.mount` and the
#      boot ends in an emergency shell. This is the failure that would reach a
#      real machine first and is the reason the test reboots rather than
#      inspecting a generation.
#   2. It comes back as *itself*. `/etc/machine-id` and `/var/lib/nixos` are
#      the two paths whose loss is silent: the machine boots fine and the
#      damage surfaces later as a stranger owning yesterday's files. Both are
#      compared across the reboot.
#   3. Unwhitelisted state dies. Without this the test passes on a machine
#      where the wipe never ran, which is the no-op the whole tier exists to
#      rule out.
#   4. Whitelisted state lives, including a per-user entry, because the
#      user-entry path restates `home` and `group` (see the module's comment
#      on `userEntries`) and a restated fact needs a check that it is equal.
#   5. The undo button works. A path that should have been whitelisted is
#      recoverable from `/old_roots` for `keepGenerations` boots, and that is
#      the entire argument for why turning this on is not a one-way door.
#
# THAT IT DISCRIMINATES, checked rather than assumed. A test of a destructive
# mechanism has to fail when the mechanism is off, and fail in the right place:
# one that fails early fails for some unrelated reason and proves nothing.
# Run on 2026-08-06 with `rollback.method = "none"` below and nothing else
# changed. The three wipe-independent subtests still passed, and the run failed
# on the first assertion of `unwhitelisted state is gone`:
#
#   command `test -e /root/junk-file` unexpectedly succeeded
#
# HOW THE FILESYSTEM GETS MADE. `systemd.repart` in the initrd, which is the
# mechanism nixpkgs' own `non-default-filesystems.nix` btrfs case uses. The
# test does not install a machine; it declares the subvolume layout and lets
# repart create it on first boot. The blank snapshot every later boot returns
# to is taken by `ephemeral-root-test-blank` below, standing in for the step a
# real installer would do once.
#
# WHAT THIS TEST DELIBERATELY DOES NOT CLAIM. It does not use `/etc` to show
# that managed configuration cannot be overridden. NixOS repopulates `/etc`
# from the generation on every activation, wipe or no wipe, so an assertion
# there would pass identically with the rollback disabled and would be
# measuring activation rather than this module. The property is real, but the
# thing that demonstrates it is subtest 3: state outside a managed path is
# what only the wipe removes.
{
  ix,
  pkgs,
  paths,
}: let
  disk = "/dev/vda";
  partition = "/dev/disk/by-label/ephemeral";
in
  pkgs.testers.runNixOSTest {
    name = "ephemeral-root";

    # The module under test reads the repo's cross-module helper bundle
    # (`ix.writePythonApplication` for `ix-wipe-preview`); an image build
    # injects it through `evalImageConfig`'s specialArgs, so the test node
    # does the same.
    node.specialArgs.ix = ix;

    nodes.machine = {config, ...}: {
      imports = [(paths.modules + "/system/ephemeral-root")];

      virtualisation = {
        rootDevice = disk;
        useDefaultFilesystems = false;
        useBootLoader = false;

        # `qemu-vm.nix` defines `fileSystems = mkVMOverride cfg.fileSystems`,
        # and priority 10 replaces the attribute set rather than merging into
        # it. So every bind the module declares is dropped inside a VM, and
        # the first run to get this far booted a machine whose whitelist
        # mounted nothing while `/proc/mounts` looked otherwise ordinary
        # (2026-08-06). Merging the module's own `bindMounts` back in is what
        # makes the thing under test the thing that runs; restating the binds
        # here would test this file instead.
        fileSystems =
          config.system.ephemeralRoot.bindMounts
          // {
            "/" = {
              device = partition;
              fsType = "btrfs";
              options = ["subvol=/root"];
            };
            "/persistent" = {
              device = partition;
              fsType = "btrfs";
              options = ["subvol=/persistent"];
              # The module asserts this, because its initrd seed writes here
              # before stage 2 exists.
              neededForBoot = true;
            };
            "/old_roots" = {
              device = partition;
              fsType = "btrfs";
              options = ["subvol=/old_roots"];
            };
          };
      };

      boot.initrd = {
        # The store stays on the driver's own mount rather than on a
        # subvolume. Putting it on the btrfs would test the driver's store
        # handling, not this module, and a store that vanished with the root
        # would fail for a reason no real machine has.
        supportedFilesystems = ["btrfs"];

        systemd = {
          enable = true;
          repart = {
            enable = true;
            device = disk;
            empty = "allow";
          };

          # Stands in for the installer. A real machine takes this snapshot
          # once, against a root that has never been booted; here it is taken
          # on first boot and reused, which is the same thing because repart
          # has just created `/root` empty.
          #
          # Ordered between repart (which creates the subvolumes) and the
          # rollback (which needs the snapshot to already exist). Getting this
          # order wrong makes the first boot fail rather than pass quietly,
          # which is the right way round.
          services.ephemeral-root-test-blank = {
            description = "Take the blank snapshot an installer would have taken";
            wantedBy = ["initrd-root-fs.target"];
            # `systemd-repart.service` creates the subvolumes; the target is
            # when the device they live on is actually there. repart finishing
            # does not imply the by-label symlink exists yet, and without the
            # target this unit failed with "special device
            # /dev/disk/by-label/ephemeral does not exist" (2026-08-06).
            after = ["systemd-repart.service" "initrd-root-device.target"];
            before = ["ephemeral-root-rollback.service"];
            unitConfig.DefaultDependencies = "no";
            serviceConfig = {
              Type = "oneshot";
              # Same reason as the rollback unit: a oneshot that does not
              # remain after exit is re-pulled by the switch-root isolate and
              # runs twice.
              RemainAfterExit = true;
            };
            script = ''
              set -euo pipefail
              mkdir -p /blank-top
              mount -t btrfs -o subvolid=5 ${partition} /blank-top
              if [ ! -e /blank-top/root-blank ]; then
                btrfs subvolume snapshot -r /blank-top/root /blank-top/root-blank
              fi
              umount /blank-top
            '';
          };
        };
      };

      systemd.repart.partitions."00-root" = {
        Type = "linux-generic";
        Format = "btrfs";
        Label = "ephemeral";
        # No `/nix`: see above. `/old_roots` is where the rollback moves the
        # displaced root, so it has to exist before the first rollback runs.
        Subvolumes = ["/root" "/persistent" "/old_roots"];
        MakeDirectories = ["/root" "/persistent" "/old_roots"];
      };

      users.users.alice = {
        isNormalUser = true;
        uid = 1000;
        group = "users";
      };

      system.ephemeralRoot = {
        enable = true;
        rollback = {
          method = "btrfs";
          device = partition;
        };
        entries = [
          {path = "/var/lib/kept-by-the-whitelist";}
          # Real state a bare NixOS declares through `StateDirectory=`. Named
          # here rather than in `ephemeralStateDirectories` so the whitelist
          # path is what the test exercises.
          {path = "/var/lib/systemd/linger";}
        ];
        users.alice.entries = [{path = ".kept";}];
        # dhcpcd's lease database is genuinely fine to lose, and saying so is
        # what the audit asks for. That this line is required at all is the
        # audit working: without it the build fails naming the directory.
        ephemeralStateDirectories = ["dhcpcd" "systemd/rfkill"];
      };
    };

    testScript = ''
      machine.start()
      machine.wait_for_unit("multi-user.target")

      # SUBTEST 0: the layout is what the module was configured against. If
      # this is wrong every later assertion is measuring something else.
      with subtest("the root is a btrfs subvolume and the whitelist is mounted"):
          # Printed before the assertions, not after: a bare `grep` that
          # returns 1 says a pattern did not match and says nothing about
          # what was there instead, which is the whole content of the answer.
          mounts = machine.succeed("cat /proc/mounts")
          print("=== /proc/mounts ===\n" + mounts)
          # `execute`, not `succeed`: these are diagnostics for whichever
          # assertion below fails, and a diagnostic that can itself fail the
          # test replaces the real error with its own.
          print("=== lsblk -f ===\n" + machine.execute("lsblk -f")[1])
          print("=== initrd rollback journal ===\n" + machine.execute(
              "journalctl -b -u ephemeral-root-rollback.service --no-pager"
          )[1])
          # /proc/mounts carries the resolved device node, not the by-label
          # symlink the config names, so the pattern has to resolve it too.
          realdev = machine.succeed("realpath ${partition}").strip()
          print("=== ${partition} resolves to " + realdev)
          machine.succeed(f"grep -E '{realdev} / btrfs .*subvol=/root ' /proc/mounts")
          machine.succeed(f"grep -E '{realdev} /persistent btrfs .*subvol=/persistent ' /proc/mounts")
          # Every whitelisted path is a mount, which is both how the wipe
          # spares it and how `ix-wipe-preview` finds it.
          for path in ["/var/lib/nixos", "/var/lib/kept-by-the-whitelist", "/home/alice/.kept"]:
              machine.succeed(f"mountpoint -q {path}")
          # /etc/ssh is the symlink case: not a mount, points at the
          # persistent copy. Asserted because the module picks `symlink` here
          # for a specific reason (rename-over returns EBUSY on a bind mount)
          # and a silent flip back to `bind` would break host keys, not this.
          machine.fail("mountpoint -q /etc/ssh")
          machine.succeed("test /persistent/etc/ssh = \"$(readlink /etc/ssh)\"")

      # The identity that must cross the reboot unchanged.
      machine_id = machine.succeed("cat /etc/machine-id").strip()
      assert machine_id, "machine-id is empty, so the bind mount produced a blank file"
      alice_uid = machine.succeed("id -u alice").strip()

      with subtest("the preview names the doomed file and not the kept one"):
          # Written before the preview runs so it has something true to say.
          machine.succeed("echo doomed > /root/junk-file")
          machine.succeed("mkdir -p /var/lib/kept-by-the-whitelist")
          machine.succeed("echo kept > /var/lib/kept-by-the-whitelist/file")
          preview = machine.succeed("ix-wipe-preview")
          print("=== ix-wipe-preview ===")
          print(preview)
          assert "/root/junk-file" in preview, (
              f"the preview did not list a file that is about to die\n{preview}"
          )
          assert "/var/lib/kept-by-the-whitelist/file" not in preview, (
              "the preview listed a whitelisted file as doomed, so -xdev did not"
              f" stop at the bind mount\n{preview}"
          )

      # Everything the reboot is supposed to decide between.
      machine.succeed("echo kept > /home/alice/.kept/file")
      machine.succeed("chown -R alice:users /home/alice/.kept")
      machine.succeed("mkdir -p /var/lib/unmanaged-service && echo doomed > /var/lib/unmanaged-service/state")
      machine.succeed("echo doomed > /etc/hand-edited.conf")

      machine.shutdown()
      machine.start()
      machine.wait_for_unit("multi-user.target")

      # SUBTEST 1 is implicit and is the most important one in the file: the
      # two lines above returned, so the rollback did not strand the boot.
      # Said out loud because a reader skimming subtests would not see it.
      with subtest("the machine came back as itself"):
          assert machine.succeed("cat /etc/machine-id").strip() == machine_id, (
              "machine-id changed across the wipe: /etc/machine-id did not persist,"
              " and every service keyed on it now sees a different machine"
          )
          assert machine.succeed("id -u alice").strip() == alice_uid, (
              "alice's uid changed across the wipe: /var/lib/nixos did not persist,"
              " so every file she owns on /persistent now belongs to a stranger"
          )

      with subtest("unwhitelisted state is gone"):
          # The discriminator. With the rollback disabled these three files
          # are all still here and this is the subtest that fails.
          machine.fail("test -e /root/junk-file")
          machine.fail("test -e /var/lib/unmanaged-service/state")
          machine.fail("test -e /etc/hand-edited.conf")

      with subtest("whitelisted state survived"):
          machine.succeed("test kept = \"$(cat /var/lib/kept-by-the-whitelist/file)\"")
          machine.succeed("test kept = \"$(cat /home/alice/.kept/file)\"")
          # Ownership is the half that a bind mount gets right for free and a
          # hand-rolled seed gets wrong. Checked because the per-user path
          # restates `group` rather than reading it from `users.users`.
          machine.succeed("test alice = \"$(stat -c %U /home/alice/.kept/file)\"")
          machine.succeed("test users = \"$(stat -c %G /home/alice/.kept/file)\"")

      with subtest("the displaced root is recoverable"):
          # The undo button, and the reason enabling this is not a one-way
          # door.
          #
          # Not a count. Every boot displaces a root, the first one included:
          # it displaces the empty subvolume repart had just made. So the
          # number of entries is the number of boots, and asserting on it only
          # restates the boot count. What has to be true is that the file the
          # wipe took is in exactly one of them, which is also what tells a
          # real displaced root from an empty directory with a plausible name.
          old_roots = machine.succeed("ls -1 /old_roots").split()
          carrying = [
              stamp
              for stamp in old_roots
              if machine.execute(f"test -e /old_roots/{stamp}/root/junk-file")[0] == 0
          ]
          assert len(carrying) == 1, (
              f"expected the wiped file in exactly one displaced root, "
              f"found it in {carrying} of {old_roots}"
          )
          recovered = machine.succeed(f"cat /old_roots/{carrying[0]}/root/junk-file").strip()
          assert recovered == "doomed", (
              f"the displaced root does not contain the file the wipe took: {recovered!r}"
          )

      with subtest("nothing failed on the way back up"):
          # A rollback that half-worked can still reach multi-user.target with
          # a failed unit behind it, which is the shape of a bug that only
          # bites on the boot after next.
          machine.succeed("systemctl is-active ephemeral-root-seed.service")
          failed = machine.succeed("systemctl --failed --no-legend --plain").strip()
          assert not failed, f"units failed across the wipe:\n{failed}"
    '';
  }
