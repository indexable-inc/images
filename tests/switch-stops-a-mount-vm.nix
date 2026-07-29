# Updating a machine must not stop the mount the update is standing on.
#
# `dac64977` moved guest /tmp off a tmpfs. The change is right, but applying
# it to a machine that was already running never completed. Nothing recorded
# that, because every check in this repo is eval-time: an assertion can read
# the generation that would be built, it cannot watch a switch into it. This
# is the first test that runs one.
#
# The chain, each link verified in the test below rather than asserted here:
#
#   1. The new generation does not declare /tmp, so switch-to-configuration
#      stops `tmp.mount` -- correct behaviour for a removed unit.
#   2. nixpkgs gives the system bus `RequiresMountsFor = [ "/tmp" ]`
#      (nixos/modules/services/system/dbus.nix, under the comment "We get
#      errors when reloading the dbus-broker service if /tmp got remounted
#      after this service started"). `RequiresMountsFor=` is `Requires=` plus
#      `After=`, and only the ordering half was wanted; the requirement half
#      means systemd stops the bus whenever it stops the mount.
#   3. switch-to-configuration drives that transaction over that bus, and is
#      blocked waiting for the stop job when its own connection goes away. It
#      exits 1 having stopped the mount and nothing else.
#
# So `/run/current-system` never advances, the activation phase never runs,
# and the rootfs /tmp underneath is left at mode 0555 with no tmpfiles rule to
# come along and fix it. Three hyperion guests ended up in that state and were
# repaired by hand (ENG-11080).
#
# WHAT THIS TEST IS FOR. Not the tmpfs, which is a one-time migration. The
# durable property is the one in the title: a switch that removes ANY mount
# unit has to survive, because the system bus is not allowed to be collateral
# damage of a config change. Anything reintroducing a `Requires=`-strength
# dependency from the bus to a mount fails here.
#
# WHY THE TMPFS IS DECLARED WITH `systemd.mounts` AND NOT `boot.tmp.useTmpfs`.
# A guest gets its tmpfs purely as a systemd mount unit. A test VM would not:
# `nixos/modules/virtualisation/qemu-vm.nix` turns `useTmpfs` into a
# `fileSystems."/tmp"` entry with `neededForBoot = true`, which puts the mount
# in fstab, behind the initrd, and under `local-fs.target`. That is a second
# and much wider cascade that no guest has, and it would let this test pass or
# fail for a reason the thing it guards does not share.
{
  lib,
  pkgs,
  paths,
}:
pkgs.testers.runNixOSTest {
  name = "switch-stops-a-mount";

  nodes.machine = {config, ...}: {
    # The fix under test, imported rather than restated. It is its own module
    # precisely so this test and `lib/image/default.nix` can share one copy;
    # a test carrying its own version of the fix would guard nothing.
    imports = [(paths.modules + "/system/dbus-survives-mount-removal.nix")];

    # Generation A declares the tmpfs and generation B does not. Expressed as
    # an option rather than as two `systemd.mounts` lists, because taking an
    # element back out of a list in a specialisation needs `mkForce`.
    options.test.tmpfsTmp = lib.mkOption {
      description = "Whether this generation mounts a tmpfs on /tmp.";
      type = lib.types.bool;
      default = true;
    };

    config = {
      systemd.mounts = lib.optional config.test.tmpfsTmp {
        what = "tmpfs";
        where = "/tmp";
        type = "tmpfs";
        # An ix guest reaches this mount through local-fs.target too, and a
        # `Wants=` edge does not propagate a stop, so it cannot be the thing
        # that takes the bus down.
        wantedBy = ["local-fs.target"];
        mountConfig.Options = "mode=1777,strictatime,rw,nosuid,nodev,size=64M";
      };

      specialisation.rootfsTmp.configuration.test.tmpfsTmp = false;
    };
  };

  testScript = ''
    machine.wait_for_unit("multi-user.target")

    # The starting state this is all about: /tmp is the tmpfs, and it works.
    machine.succeed("systemctl is-active tmp.mount")
    machine.succeed("test tmpfs = \"$(findmnt -no FSTYPE /tmp)\"")

    # The two halves of the fix, on the unit systemd actually loaded rather
    # than on the Nix that produced it. The ordering nixpkgs' comment asked
    # for has to survive; the requirement that killed the machine must not.
    #
    # `-P`, not `-p`. `-p` prints `After=a b c`, so splitting on spaces makes
    # the first entry read `After=a` and an exact-match grep silently depends
    # on where systemd happens to have put tmp.mount in an unordered set. That
    # passed locally and failed in CI on the identical derivation, which is
    # the whole tell: a check whose answer depends on set order is not a check.
    machine.succeed(
        "systemctl show dbus-broker.service -P After | tr ' ' '\\n' | grep -qx tmp.mount"
    )
    machine.fail(
        "systemctl show dbus-broker.service -P Requires | tr ' ' '\\n' | grep -qx tmp.mount"
    )

    # Recorded before anything is touched, so a failure here names its own
    # cause instead of leaving the next reader to rediscover it.
    print("=== units that stop when tmp.mount stops ===")
    print(machine.succeed("systemctl list-dependencies --reverse --plain tmp.mount || true"))

    before = machine.succeed("readlink -f /run/current-system").strip()

    switch = "/run/current-system/specialisation/rootfsTmp/bin/switch-to-configuration"
    rc, out = machine.execute(f"{switch} switch 2>&1")
    print(f"=== switch output (exit {rc}) ===")
    print(out)

    # The failure this test exists for, named exactly as the guests reported it.
    assert "disconnected from D-Bus" not in out, (
        f"the switch lost its own bus connection stopping tmp.mount (ENG-11080)\n{out}"
    )

    # Getting into the activation phase is the property that matters most, and
    # not for its own sake: activation is what applies systemd's `q /tmp 1777`
    # tmpfiles rule. A switch that dies in the stop phase leaves the rootfs
    # /tmp the unmount exposed at 0555 with nothing coming to fix it, which is
    # what made three guests unwritable.
    assert "activating the configuration" in out, (
        f"the switch never reached activation, so nothing repairs /tmp\n{out}"
    )

    # The bus survived, and answers rather than merely being marked active.
    machine.succeed("systemctl is-active dbus-broker.service")
    machine.succeed("busctl --system --no-pager status >/dev/null")
    machine.succeed("touch /tmp/written-after-the-switch")

    # And the switch recorded what it did. The original failure aborted before
    # this, so `ix apply` reported failure on a guest that had not moved.
    after = machine.succeed("readlink -f /run/current-system").strip()
    assert after != before, (
        f"the switch left /run/current-system at {before}, so nothing was recorded"
    )

    # WHY THE EXIT CODE IS NOT ASSERTED, AND WHAT THAT LEAVES UNCOVERED.
    #
    # `Failed to stop tmp.mount` and exit 4 are unavoidable under the test
    # driver, for two reasons neither of which a guest has. The backdoor shell
    # every command runs through does `cd /tmp`
    # (nixos/modules/testing/test-instrumentation.nix), and each command runs
    # in a subshell, so its working directory cannot be moved out of the way.
    # The driver also 9p-mounts /tmp/xchg and /tmp/shared underneath, and a
    # mount with submounts cannot be unmounted at all. Measured in this VM:
    # `umount /tmp` answers `target is busy` even with nothing else running.
    #
    # So this test holds the part that made the failure catastrophic -- the bus
    # dies, activation never runs, nothing is recorded -- and cannot hold the
    # part that makes an apply merely *report* failure. Nonzero is nonzero to
    # the node agent, so the exit code still needs closing; ENG-11080 carries
    # why it cannot be closed from the new generation and what a mount being
    # retired has to carry in the generation before.
    #
    # Instead of the exit code, assert the failure surface: the unmount is the
    # only thing allowed to have failed. A regression that took anything else
    # down shows up here rather than hiding behind an exit code this test had
    # to tolerate.
    failed = [line for line in out.splitlines() if line.startswith("Failed ")]
    assert failed == ["Failed to stop tmp.mount"], (
        f"the switch failed at more than the unmount: {failed}\n{out}"
    )
  '';
}
