# The `lvmthin` wipe fires on a real LVM thin pool, and the root LV's device
# node survives being destroyed and re-created underneath `sysroot.mount`.
#
# `modules/system/ephemeral-root` has three `rollback.method` values.
# `tests/ephemeral-root-vm.nix` boots the `btrfs` one. This file exists because
# `lvmthin` could not be reached from there and had never booted anywhere:
# every claim in that script was argued from `lvcreate(8)` and none was
# measured.
#
# WHY A DIFFERENT HARNESS AND NOT ANOTHER `runNixOSTest` NODE. `qemu-vm.nix`
# defines `fileSystems = mkVMOverride cfg.fileSystems`, priority 10, which
# replaces the attribute set rather than merging into it. The btrfs test works
# around that by merging `config.system.ephemeralRoot.bindMounts` back into
# `virtualisation.fileSystems` and declaring the root itself there, which is
# fine when the layout is something the test invents on first boot with
# `systemd.repart`. It is not fine here. `lvmthin` needs a volume group with a
# thin pool, a `root` LV and a separately mkfs'd `root-blank` LV all present
# before the initrd starts, and it needs `fileSystems."/"` to name the root LV
# so that systemd derives a `.device` unit for it -- which is the entire subject
# of the device-unit subtest below. `disko.lib.testLib.makeDiskoTest` partitions
# a blank disk, installs the system onto it, and then boots that disk in a VM whose
# NixOS closure is an ordinary `eval-config` (no `qemu-vm.nix`, so `fileSystems`
# merges the way it does on a real machine). That boot is the only reason this
# repo has a disko input at all; see the comment on it in `flake.nix`.
#
# WHAT THIS TEST HOLDS, in order of how expensive the failure would be:
#
#   1. THE DEVICE UNIT. Stated by the author of the `lvmthin` script as the one
#      thing its shape does that btrfs does not, and as a state no test had
#      produced: the rollback renames `vg0/root` aside and creates a new LV
#      under that name, which destroys and re-creates the device node that
#      `sysroot.mount` carries a `Requires=` on. Ordering says the node should
#      be back in time. Two initrd probe units on either side of the rollback
#      record `dev-vg0-root.device` from systemd's own point of view, and
#      `the root LV's device unit went away and came back` asserts that the
#      unit was already active before the rollback, that its backing
#      `SysFSPath` is a *different* dm device afterwards, and
#      that it is active again. That is the difference between "the boot
#      happened to work" and "the state everyone was worried about was entered
#      and left".
#   2. THE ACTIVATION-SKIP FLAG. The script passes `--setactivationskip n` and
#      then `lvchange --activate y` because `lvcreate(8)` says a thin snapshot
#      gets the activation-skip flag by default. That subtest measures both:
#      the root LV the rollback produced must NOT carry `k`, and a snapshot this
#      test creates *without* the flag must carry it. The second assertion is
#      what makes the module's justification falsifiable rather than decorative
#      -- if lvm2 ever stops setting `k` by default, this fails and says so
#      instead of leaving two dead flags in an initrd script.
#   3. The machine comes back at all, and comes back as itself. Same argument as
#      the btrfs test: `/etc/machine-id` and `/var/lib/nixos` are the two paths
#      whose loss is silent.
#   4. Unwhitelisted state dies and whitelisted state lives, across a real
#      reboot of the same disk image.
#   5. The undo button. `keepGenerations = 1` here rather than the default 3, so
#      two boots are enough to watch `lvremove` actually prune: after the second
#      boot exactly one `old-root-*` LV may remain, and it must be the one
#      holding the file the wipe took.
#
# NEGATIVE CONTROLS. Every subtest below was run once with the behaviour it
# measures switched off, on a copy of this tree, and every failure text here is
# pasted out of that run's driver log rather than described. A subtest that
# cannot be made to fail is not measuring anything.
#
#   (a) `the root LV's device unit went away and came back`, with both probe
#       units moved to the same side of the rollback
#       (`ephemeral-root-device-probe-before` given
#       `after = ["ephemeral-root-rollback.service"]`):
#
#         AssertionError: dev-vg0-root.device is backed by the same dm device
#         before and after the rollback (/sys/devices/virtual/block/dm-7), so
#         the node was never destroyed and this subtest measured nothing
#
#   (b) `the activation-skip flag is set by default and cleared here`, first
#       half, with `--setactivationskip n` deleted from the module's `lvcreate`
#       and `lvchange --activate y` deliberately LEFT IN. The assertion is
#       never reached, because the machine does not come up: an LV carrying `k`
#       is skipped by a plain `lvchange -ay` too (clearing it needs `-K`), so
#       the root LV exists and is inactive and there is no device node.
#       Verbatim, from the initrd console and then the driver:
#
#         systemd[1]: systemd-fsck-root.service: Bound to unit
#         dev-vg0-root.device, but unit isn't active.
#         systemd[1]: Dependency failed for /sysroot.
#         systemd[1]: Reached target Emergency Mode.
#         ...
#         RuntimeError: Shell disconnected
#
#       That is the emergency shell the module's comment predicts, produced on
#       purpose. It also settles something the module could not: the `lvchange`
#       is not a fallback for the flag, and neither line is redundant.
#
#   (c) `the activation-skip flag is set by default and cleared here`, second
#       half, with `--setactivationskip n` added to the control snapshot this
#       test creates:
#
#         AssertionError: a thin snapshot created without --setactivationskip
#         came out as Vwi-a-tz--, with no activation-skip flag. lvcreate(8) is
#         the module's whole justification for passing `--setactivationskip n`
#         and running `lvchange --activate y`; if that is no longer how lvm2
#         behaves, those two lines are doing nothing and the comment above them
#         is wrong
#
#   (d) `unwhitelisted state is gone`, with `rollback.method = "none"` (and the
#       three subtests that presuppose a rollback removed, so the run reaches
#       the reboot):
#
#         RequestedAssertionFailed: command `test -e /root/junk-file`
#         unexpectedly succeeded
#
#       the same text the btrfs test records for the same assertion.
#
#   (e) `whitelisted state survived`, with `/var/lib/kept-by-the-whitelist`
#       removed from `entries`:
#
#         RequestedAssertionFailed: command `test kept = "$(cat
#         /var/lib/kept-by-the-whitelist/file)"` failed (exit code 1)
#
#   (f) `the preview names the doomed file and not the kept one`, same mutation
#       as (e), with the layout subtest's mount check for that path removed so
#       the run reaches the preview:
#
#         AssertionError: the preview listed a whitelisted file as doomed, so
#         -xdev did not stop at the bind mount
#
#       followed by the whole `ix-wipe-preview` dump, which the assertion
#       prints on purpose and which is too long to repeat here. The two lines
#       in it that the assertion is about, and which are absent from a passing
#       run, are `/var/lib/kept-by-the-whitelist` and
#       `/var/lib/kept-by-the-whitelist/file`.
#
#   (g) `the displaced root is recoverable and older ones are pruned`, with
#       `keepGenerations = 3` instead of 1, so two boots are no longer enough
#       to make the prune run:
#
#         AssertionError: keepGenerations = 1 and two boots have happened, so
#         exactly one displaced root should be left; found
#         ['old-root-2026-08-06T13-00-34', 'old-root-2026-08-06T13-00-52']
#
#       The message still says "keepGenerations = 1" because it describes this
#       file's configuration, not the mutated one. Two LVs is the whole
#       finding: with the keep count raised, the second boot's `lvremove` had
#       nothing to take.
#
# WHAT HAS NO RECORDED NEGATIVE CONTROL, said out loud rather than implied.
# The layout subtest and the closing `nothing failed on the way back up`
# are not behaviours of this module that can be switched off in one edit --
# the layout is the precondition every other subtest is measured against, and
# the closing one is a whole-system property. `the machine came back as itself`
# has none either: `/etc/machine-id` and `/var/lib/nixos` are in the module's
# unconditional `systemSeed`, so removing them means editing the module's own
# invariant rather than the configuration under test, and the btrfs test
# already covers that path.
{
  ix,
  pkgs,
  paths,
  diskoLib,
}: let
  # One spelling of each name, because three separate things have to agree on
  # it: the disko layout that creates the LV, the module option that names it,
  # and the systemd device unit whose escaped name the device-unit subtest reads.
  volumeGroup = "vg0";
  rootVolume = "root";
  blankVolume = "root-blank";
  persistentVolume = "persistent";
  thinPool = "thinpool";

  # `utils.escapeSystemdPath "/dev/vg0/root"`, written out rather than derived:
  # this test is the thing that proves the derivation in the module is right, so
  # it must not share the helper. Every segment here is `[a-z0-9-]` with no dot
  # and no dash inside a segment, which is the shape where the escape is a plain
  # `/` -> `-` substitution.
  rootDeviceUnit = "dev-${volumeGroup}-${rootVolume}.device";

  # Written by the two initrd probe units below. `/run` and not the persistent
  # filesystem: systemd's switch-root moves the initramfs `/run` into the new
  # root, so a file written here in the initrd is readable in stage 2, and it
  # cannot be confused with a leftover from the previous boot the way anything
  # under `/persistent` could.
  probeDir = "/run/ephemeral-root-device-probe";

  # Both probes record the same fields so the test can diff them. `systemctl
  # show` and not `is-active`: `show` exits 0 for a unit systemd has never heard
  # of and prints `ActiveState=inactive`, so the probe cannot fail the boot on
  # the exact configuration whose failure it is supposed to report.
  #
  # Written to a file AND echoed to stdout, which the initrd journal forwards to
  # the console. The file is what the assertions read; the console copy is the
  # only thing that exists if this configuration hangs before switch-root, which
  # is exactly the failure the probes are here to catch. A hang leaves nothing
  # to read `${probeDir}` with.
  probeScript = phase: ''
    set -euo pipefail
    mkdir -p ${probeDir}
    systemctl show ${rootDeviceUnit} \
      --property=Id \
      --property=ActiveState \
      --property=SysFSPath \
      --property=ActiveEnterTimestampMonotonic \
      > ${probeDir}/${phase}
    echo "=== probe ${phase}: ${rootDeviceUnit} ==="
    cat ${probeDir}/${phase}

    # Whether the root mount still has a job at all. A device unit that goes
    # away takes `systemd-fsck-root.service` with it, and `sysroot.mount`
    # requires that unit, so the mount's queued job can be dropped without any
    # unit failing -- a boot that stops rather than one that errors. No `sed`
    # or `grep` anywhere in here: the initrd's PATH is coreutils, systemd, kmod
    # and bash, and nothing else unless a module puts it there.
    echo "=== probe ${phase}: sysroot.mount ==="
    systemctl show sysroot.mount \
      --property=ActiveState \
      --property=Result \
      --property=Requires \
      --property=After
    echo "=== probe ${phase}: jobs ==="
    systemctl list-jobs --no-pager --no-legend
  '';

  mkProbe = {
    phase,
    after,
    before,
  }: {
    description = "Record ${rootDeviceUnit} ${phase} the rollback";
    wantedBy = ["initrd-root-fs.target"];
    inherit after before;
    unitConfig.DefaultDependencies = "no";
    serviceConfig = {
      Type = "oneshot";
      # Same reason the module's own initrd oneshots set it: without it the
      # switch-root isolate pulls the unit in a second time, and the second run
      # would overwrite the record with a reading taken after the rollback.
      RemainAfterExit = true;
    };
    script = probeScript phase;
  };
in
  diskoLib.testLib.makeDiskoTest {
    inherit pkgs;
    name = "ephemeral-root-lvmthin";

    # `root-blank` is an ordinary thin LV with a filesystem and no mountpoint,
    # which is exactly what the module's `blankVolume` documents: mkfs'd once at
    # install time and never mounted read-write. disko formats it and then
    # leaves it alone, so this is the installer step a real machine does once,
    # expressed as part of the layout rather than as a script.
    #
    # Sizes are bounded by the harness: `makeDiskoTest` gives each disk 4096 MiB
    # and that is not an argument it takes.
    disko-config = {
      disko.devices = {
        disk.main = {
          type = "disk";
          # Overwritten by `testLib.prepareDiskoConfig`, which renumbers every
          # disk to the qemu device it actually gets. Required by the type.
          device = "/dev/vdb";
          content = {
            type = "gpt";
            partitions = {
              ESP = {
                size = "512M";
                type = "EF00";
                content = {
                  type = "filesystem";
                  format = "vfat";
                  mountpoint = "/boot";
                  mountOptions = ["umask=0077"];
                };
              };
              pv = {
                size = "100%";
                content = {
                  type = "lvm_pv";
                  vg = volumeGroup;
                };
              };
            };
          };
        };
        lvm_vg.${volumeGroup} = {
          type = "lvm_vg";
          lvs = {
            # disko orders LV creation by a priority derived from the type, and
            # a thin pool sorts before a plain LV, so the pool exists before the
            # two thin volumes below name it. Both sizes are literal rather than
            # percentages: a `%` size sorts last, which would hand the whole
            # volume group to whichever of these asked for it first.
            ${thinPool} = {
              size = "2G";
              lvm_type = "thin-pool";
            };
            ${persistentVolume} = {
              size = "512M";
              content = {
                type = "filesystem";
                format = "ext4";
                mountpoint = "/persistent";
              };
            };
            ${rootVolume} = {
              size = "1G";
              lvm_type = "thinlv";
              pool = thinPool;
              content = {
                type = "filesystem";
                format = "ext4";
                mountpoint = "/";
              };
            };
            ${blankVolume} = {
              size = "1G";
              lvm_type = "thinlv";
              pool = thinPool;
              content = {
                type = "filesystem";
                format = "ext4";
              };
            };
          };
        };
      };
    };

    extraSystemConfig = {...}: {
      imports = [(paths.modules + "/system/ephemeral-root")];

      # `makeDiskoTest` has no specialArgs seam, so the module's `ix`
      # argument (the cross-module helper bundle an image build injects
      # through `evalImageConfig`) arrives as an ordinary module arg instead.
      _module.args.ix = ix;

      # The module asserts this, and says why: the scripted initrd drops
      # `boot.initrd.systemd.services.*` silently, so without it the machine
      # boots, keeps everything, and looks exactly like one where the wipe
      # works.
      boot.initrd.systemd.enable = true;

      # disko's generated `fileSystems."/persistent"` does not know this and the
      # module asserts it: the initrd seed writes `/etc/machine-id` and
      # `/var/lib/nixos` under here before stage 2 exists. Set as a leaf on the
      # existing entry so it merges with disko's rather than replacing it.
      fileSystems."/persistent".neededForBoot = true;

      # The booted machine has no NIC: `makeDiskoTest` builds its own qemu
      # command line and passes no `-netdev`. Left on, `dhcpcd` would sit in
      # `multi-user.target`'s dependency set waiting for an interface that
      # cannot arrive, and the first `wait_for_unit` would time out on
      # something that has nothing to do with this module.
      networking.useDHCP = false;

      users.users.alice = {
        isNormalUser = true;
        uid = 1000;
        group = "users";
      };

      system.ephemeralRoot = {
        enable = true;
        rollback = {
          method = "lvmthin";
          inherit volumeGroup rootVolume blankVolume;
          # One, not the default three, and this is the only reason two boots
          # are enough to see the prune run. With the default, both boots would
          # be under the keep count and `lvremove` would never be reached, so
          # the retention subtest would be asserting that nothing happened.
          keepGenerations = 1;
        };
        entries = [
          {path = "/var/lib/kept-by-the-whitelist";}
          # Real state a bare NixOS declares through `StateDirectory=`. Kept
          # rather than declared ephemeral so the whitelist is what carries it.
          {path = "/var/lib/systemd/linger";}
        ];
        users.alice.entries = [{path = ".kept";}];
        # The audit demands a decision on every `StateDirectory=` in the
        # generation. That this is named at all is the audit working: without
        # the line the build fails naming the directory.
        ephemeralStateDirectories = ["systemd/rfkill"];
      };

      # THE PROBES, and they are the point of the file.
      #
      # They are here and not in the module because they measure the module.
      # `probe-before` runs after `initrd-root-device.target`, which is the
      # target systemd's fstab-generator hangs the root device unit off, so by
      # the time it writes its record systemd has already seen
      # `dev-vg0-root.device` and brought it up. `probe-after` runs after the
      # rollback and before `sysroot.mount`, which is the window the module's
      # comment says is the risky one.
      boot.initrd.systemd.services = {
        ephemeral-root-device-probe-before = mkProbe {
          phase = "before";
          after = ["initrd-root-device.target"];
          before = ["ephemeral-root-rollback.service"];
        };
        ephemeral-root-device-probe-after = mkProbe {
          phase = "after";
          # The rollback service is named twice over: once as the thing to run
          # after, and once through the before-probe. `After=` on a unit that
          # does not exist is satisfied immediately, so with the rollback
          # disabled -- which is negative control (a) -- ordering against the
          # rollback alone would leave the two probes free to run in either
          # order and the control would prove nothing.
          after = [
            "ephemeral-root-rollback.service"
            "ephemeral-root-device-probe-before.service"
          ];
          before = ["sysroot.mount"];
        };
      };
    };

    extraTestScript = ''
      machine.wait_for_unit("multi-user.target")

      def probe(phase):
          """systemd's own view of ${rootDeviceUnit}, as recorded inside the
          initrd on one side of the rollback."""
          raw = machine.succeed(f"cat ${probeDir}/{phase}")
          print(f"=== {phase} the rollback ===\n" + raw)
          return dict(
              line.split("=", 1) for line in raw.strip().splitlines() if "=" in line
          )

      # THE LAYOUT: the layout is the one the module was configured against. If
      # this is wrong every later assertion is measuring something else.
      with subtest("the root is a thin LV snapshotted from the blank volume"):
          # Printed before the assertions, not after. A bare `grep` that returns
          # 1 says a pattern did not match and says nothing about what was
          # there instead, which is the whole content of the answer.
          print("=== /proc/mounts ===\n" + machine.succeed("cat /proc/mounts"))
          # `execute`, not `succeed`: these are diagnostics for whichever
          # assertion below fails, and a diagnostic that can itself fail the
          # test replaces the real error with its own.
          print("=== lvs ===\n" + machine.execute(
              "lvs -a -o lv_name,lv_attr,pool_lv,origin,lv_size"
          )[1])
          print("=== initrd rollback journal ===\n" + machine.execute(
              "journalctl -b -u ephemeral-root-rollback.service --no-pager"
          )[1])

          # The root really is the LV the layout made, resolved on both sides
          # because /proc/mounts carries a device node and the config names a
          # symlink.
          root_source = machine.succeed("findmnt -no SOURCE /").strip()
          root_node = machine.succeed(f"readlink -f {root_source}").strip()
          lv_node = machine.succeed(
              "readlink -f /dev/${volumeGroup}/${rootVolume}"
          ).strip()
          assert root_node == lv_node, (
              f"/ is mounted from {root_source} ({root_node}) but"
              f" /dev/${volumeGroup}/${rootVolume} is {lv_node}, so the wipe and"
              " the root are not the same volume"
          )

          # A thin volume (`V`), and a snapshot of the blank one. Together these
          # are what make `lvcreate --snapshot` without a size legal at all, and
          # the origin is the difference between a root the rollback produced
          # and the root disko installed.
          attr = machine.succeed(
              "lvs --noheadings -o lv_attr ${volumeGroup}/${rootVolume}"
          ).strip()
          assert attr.startswith("V"), (
              f"${volumeGroup}/${rootVolume} has lv_attr {attr!r}, which is not a"
              " thin volume, so nothing here snapshotted anything"
          )
          origin = machine.succeed(
              "lvs --noheadings -o origin ${volumeGroup}/${rootVolume}"
          ).strip()
          assert origin == "${blankVolume}", (
              f"the root LV's origin is {origin!r}, not '${blankVolume}': the"
              " running root is not a snapshot of the blank volume, so the"
              " rollback did not produce it"
          )

          # Same resolve-both-sides shape as the root above, and for the same
          # reason: `/dev/mapper/<vg>-<lv>` and `/dev/<vg>/<lv>` are both
          # symlinks to the same `/dev/dm-N`, so comparing either name against
          # a resolved one is a comparison that can never be true.
          persistent_node = machine.succeed(
              "readlink -f \"$(findmnt -no SOURCE /persistent)\""
          ).strip()
          persistent_lv_node = machine.succeed(
              "readlink -f /dev/${volumeGroup}/${persistentVolume}"
          ).strip()
          assert persistent_node == persistent_lv_node, (
              f"/persistent is mounted from {persistent_node} but"
              f" /dev/${volumeGroup}/${persistentVolume} is"
              f" {persistent_lv_node}, so the state the whitelist keeps does not"
              " live on the volume that survives the wipe"
          )

          # Every whitelisted path is a mount, which is both how the wipe spares
          # it and how `ix-wipe-preview` finds it.
          for path in [
              "/var/lib/nixos",
              "/var/lib/kept-by-the-whitelist",
              "/home/alice/.kept",
          ]:
              machine.succeed(f"mountpoint -q {path}")
          # /etc/ssh is the symlink case: not a mount, points at the persistent
          # copy. Asserted because the module picks `symlink` here for a
          # specific reason (rename-over returns EBUSY on a bind mount) and a
          # silent flip back to `bind` would break host keys, not this.
          machine.fail("mountpoint -q /etc/ssh")
          machine.succeed("test /persistent/etc/ssh = \"$(readlink /etc/ssh)\"")

      # THE ACTIVATION-SKIP FLAG: the second unverified claim in the module, measured from both
      # ends.
      with subtest("the activation-skip flag is set by default and cleared here"):
          # Field 10 of lv_attr. `k` means the volume is skipped at activation,
          # which for the root LV is a boot that ends in the emergency shell
          # with the volume present and inactive.
          attr = machine.succeed(
              "lvs --noheadings -o lv_attr ${volumeGroup}/${rootVolume}"
          ).strip()
          assert attr[9] != "k", (
              f"the root LV carries the activation-skip flag ({attr}), so"
              " `--setactivationskip n` did not take and the next boot has no"
              " device node for sysroot.mount"
          )

          # The other half, and the reason those two lines are in the module at
          # all. The module argues from lvcreate(8) that a thin snapshot gets
          # `k` by default; this creates one without the flag and reads it back.
          # If lvm2 ever stops doing that, this fails and says so, rather than
          # leaving two unexplained lines in an initrd script forever.
          machine.succeed(
              "lvcreate --snapshot --name activation-skip-probe"
              " ${volumeGroup}/${blankVolume}"
          )
          probe_attr = machine.succeed(
              "lvs --noheadings -o lv_attr ${volumeGroup}/activation-skip-probe"
          ).strip()
          machine.succeed("lvremove --yes ${volumeGroup}/activation-skip-probe")
          assert probe_attr[9] == "k", (
              "a thin snapshot created without --setactivationskip came out as"
              f" {probe_attr}, with no activation-skip flag. lvcreate(8) is the"
              " module's whole justification for passing"
              " `--setactivationskip n` and running `lvchange --activate y`;"
              " if that is no longer how lvm2 behaves, those two lines are"
              " doing nothing and the comment above them is wrong"
          )

      # THE DEVICE UNIT: the failure mode this file was written for.
      with subtest("the root LV's device unit went away and came back"):
          before = probe("before")
          after = probe("after")

          assert before.get("ActiveState") == "active", (
              "${rootDeviceUnit} was not active before the rollback"
              f" ({before}), so systemd had not seen the device yet and the"
              " rollback never entered the state this subtest exists to"
              " measure"
          )
          assert after.get("ActiveState") == "active", (
              "${rootDeviceUnit} is not active after the rollback"
              f" ({after}). The boot got this far anyway, which means"
              " sysroot.mount was satisfied by something other than the unit it"
              " declares a Requires= on"
          )
          # A rename keeps the device-mapper device; only a fresh `lvcreate`
          # gets a new one. So a changed SysFSPath is the proof that the node
          # backing the unit was destroyed and re-created rather than relabelled
          # in place.
          assert before.get("SysFSPath") != after.get("SysFSPath"), (
              "${rootDeviceUnit} is backed by the same dm device before and"
              f" after the rollback ({before.get('SysFSPath')}), so the node was"
              " never destroyed and this subtest measured nothing"
          )
          assert (
              before.get("ActiveEnterTimestampMonotonic")
              != after.get("ActiveEnterTimestampMonotonic")
          ), (
              "${rootDeviceUnit} never left the active state across the"
              f" rollback ({before.get('ActiveEnterTimestampMonotonic')}), so"
              " systemd did not observe the device going away and this test"
              " does not hold the ordering claim it says it does"
          )

      # The identity that must cross the reboot unchanged.
      machine_id = machine.succeed("cat /etc/machine-id").strip()
      assert machine_id, "machine-id is empty, so the bind mount produced a blank file"
      alice_uid = machine.succeed("id -u alice").strip()

      with subtest("the preview names the doomed file and not the kept one"):
          machine.succeed("echo doomed > /root/junk-file")
          machine.succeed("mkdir -p /var/lib/kept-by-the-whitelist")
          machine.succeed("echo kept > /var/lib/kept-by-the-whitelist/file")
          preview = machine.succeed("ix-wipe-preview")
          print("=== ix-wipe-preview ===\n" + preview)
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
      machine.succeed(
          "mkdir -p /var/lib/unmanaged-service"
          " && echo doomed > /var/lib/unmanaged-service/state"
      )
      machine.succeed("echo doomed > /etc/hand-edited.conf")

      # The LV the next boot will displace, so the retention subtest can name
      # the one it expects to find and the one it expects to be gone.
      def old_roots():
          """The displaced roots this volume group is currently holding.

          `execute` and not `succeed`: the pipeline ends in a `grep` that exits
          1 when nothing matches, and "no old roots yet" is a real answer on the
          way in, not a failure."""
          return machine.execute(
              "lvs --noheadings -o lv_name ${volumeGroup} | tr -d ' '"
              " | grep '^old-root-'"
          )[1].split()

      displaced = old_roots()
      print(f"=== old roots before the second boot: {displaced}")

      machine.shutdown()
      machine.start()
      machine.wait_for_unit("multi-user.target")

      # THE MOST IMPORTANT ASSERTION IN THIS FILE IS IMPLICIT AND IS RIGHT HERE: the two
      # lines above returned, so a rollback that destroys and re-creates the
      # root LV's device node did not strand the boot. Said out loud because a
      # reader skimming subtests would not see it.
      with subtest("the machine came back as itself"):
          assert machine.succeed("cat /etc/machine-id").strip() == machine_id, (
              "machine-id changed across the wipe: /etc/machine-id did not"
              " persist, and every service keyed on it now sees a different"
              " machine"
          )
          assert machine.succeed("id -u alice").strip() == alice_uid, (
              "alice's uid changed across the wipe: /var/lib/nixos did not"
              " persist, so every file she owns on /persistent now belongs to a"
              " stranger"
          )

      with subtest("unwhitelisted state is gone"):
          # The discriminator. With the rollback disabled these three files are
          # all still here and this is the subtest that fails.
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

      with subtest("the displaced root is recoverable and older ones are pruned"):
          # The undo button, and the reason enabling this is not a one-way door.
          # `keepGenerations = 1`, so after two boots exactly one displaced root
          # may be left: the one this boot displaced, holding the file the wipe
          # took. Anything else means either the prune did not run or it took
          # the wrong one.
          remaining = old_roots()
          print(f"=== old roots after the second boot: {remaining}")
          assert len(remaining) == 1, (
              "keepGenerations = 1 and two boots have happened, so exactly one"
              f" displaced root should be left; found {remaining}"
          )
          for gone in displaced:
              assert gone not in remaining, (
                  f"{gone} was displaced by the first boot and should have been"
                  " pruned by the second; the prune is keeping more than"
                  " keepGenerations"
              )

          kept = remaining[0]
          machine.succeed(f"lvchange --activate y ${volumeGroup}/{kept}")
          machine.succeed("mkdir -p /mnt/old-root")
          machine.succeed(f"mount -o ro /dev/${volumeGroup}/{kept} /mnt/old-root")
          recovered = machine.succeed("cat /mnt/old-root/root/junk-file").strip()
          machine.succeed("umount /mnt/old-root")
          assert recovered == "doomed", (
              "the displaced root does not contain the file the wipe took:"
              f" {recovered!r}"
          )

      with subtest("nothing failed on the way back up"):
          # A rollback that half-worked can still reach multi-user.target with a
          # failed unit behind it, which is the shape of a bug that only bites
          # on the boot after next.
          machine.succeed("systemctl is-active ephemeral-root-seed.service")
          failed = machine.succeed("systemctl --failed --no-legend --plain").strip()
          assert not failed, f"units failed across the wipe:\n{failed}"
    '';
  }
