# Retiring a mount from a machine that is already running: the recipe, run.
#
# tests/switch-stops-a-mount-vm.nix holds the durable property -- the system bus
# must not be stopped when a mount is -- and `modules/system/
# dbus-survives-mount-removal.nix` makes that true for every generation built
# from here on. Neither reaches a guest that is running a generation built
# BEFORE the fix, because switch-to-configuration reads the running
# generation's unit files and does its `daemon-reload` only after the stop
# phase. A guest already on a tmpfs /tmp needs something done to it first, and
# this is that something, executed rather than described.
#
# The node here deliberately does NOT import the fix, so it carries nixpkgs'
# `RequiresMountsFor = [ "/tmp" ]` exactly as the guests in the field do.
#
# Three steps, and the order is the whole point:
#
#   1. Fix the mode of the /tmp DIRECTORY on the rootfs, which the tmpfs is
#      hiding. The ix base image ships it 0555, and once the tmpfs is out of the
#      way it is what /tmp becomes; the guests that hit this were left
#      unwritable for exactly that reason. Doing it after the detach leaves a
#      window where nothing on the machine can write /tmp; doing it before
#      leaves none. Reaching the hidden directory at all needs a second view of
#      the root, since /tmp resolves to the tmpfs from everywhere else.
#   2. Detach the tmpfs LAZILY. A plain `umount` returns EBUSY on a busy mount,
#      and that is what makes switch-to-configuration exit 4 on `Failed to stop
#      tmp.mount`, which the node agent reads as a failed apply however well the
#      guest actually did (the ENG-11063 shape). `umount -l` cannot fail that
#      way. Doing it before the switch rather than leaving it to the switch also
#      means the mount unit is already inactive when the switch runs, so there
#      is no stop job at all and the bus is never in the transaction.
#   3. Run the switch from a directory OUTSIDE /tmp. This one is not optional
#      and is easy to miss: a lazy detach leaves any process whose working
#      directory was inside the tmpfs unable to resolve it, and
#      switch-to-configuration shells out to things that call getcwd. Measured
#      here, skipping it fails the `etc` activation snippet with `Can't cd to :
#      No such file or directory` and exits 2 -- past the danger, but still
#      nonzero, and still a rejected apply.
#
# There is deliberately no drop-in resetting the bus's `RequiresMountsFor=`.
# Step 2 already means no stop job is issued for the mount, so a drop-in would
# be a second mechanism for one property, and the one that is harder to see
# working.
#
# The switch must exit 0. This is the only check here that gets to assert that:
# the sibling test cannot, because the test driver holds /tmp open in ways no
# guest does, and a lazy detach is exactly what steps around it.
#
# Kept as a check rather than deleted after the migration because the class
# recurs. Every future mount retirement needs this same sequence on machines
# built before the retirement, and a recipe nobody has run is a recipe that
# does not work.
{
  lib,
  pkgs,
}:
pkgs.testers.runNixOSTest {
  name = "mount-migration";

  nodes.machine = {config, ...}: {
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
        wantedBy = ["local-fs.target"];
        mountConfig.Options = "mode=1777,strictatime,rw,nosuid,nodev,size=64M";
      };

      specialisation.rootfsTmp.configuration.test.tmpfsTmp = false;
    };
  };

  testScript = ''
    machine.wait_for_unit("multi-user.target")
    machine.succeed("test tmpfs = \"$(findmnt -no FSTYPE /tmp)\"")

    # This node is the field's shape, not the fixed one. If that ever stops
    # being true the recipe below is being rehearsed against a machine that did
    # not need it.
    machine.succeed(
        "systemctl show dbus-broker.service -P Requires | tr ' ' '\\n' | grep -qx tmp.mount"
    )

    # `mount --bind /` gives a view of the root with nothing mounted over it,
    # which is the only way to reach the /tmp the tmpfs is covering.
    machine.succeed("mkdir -p /run/rootfs-view && mount --bind / /run/rootfs-view")

    # Put the hidden directory in the state the ix base image ships it in.
    # Without this the recipe's first step would be a no-op here -- a generic
    # NixOS rootfs already has /tmp at 1777, measured -- and the step that
    # actually mattered in the field would go untested.
    machine.succeed("chmod 0555 /run/rootfs-view/tmp")

    # Step 1.
    machine.succeed("chmod 1777 /run/rootfs-view/tmp")
    machine.succeed("test 1777 = \"$(stat -c %a /run/rootfs-view/tmp)\"")
    machine.succeed("umount /run/rootfs-view && rmdir /run/rootfs-view")

    # Step 2. The plain umount is tried first and recorded, because the EBUSY it
    # returns is the whole reason the lazy one is in the recipe.
    plain_rc, plain_out = machine.execute("umount /tmp 2>&1")
    print(f"=== plain umount /tmp: rc={plain_rc} {plain_out.strip()} ===")
    assert plain_rc != 0, (
        "a plain umount succeeded, so this VM is not reproducing the busy /tmp"
        " the lazy detach exists for"
    )
    machine.succeed("umount -l /tmp")
    machine.succeed("test -z \"$(findmnt -no FSTYPE /tmp)\"")

    # The window step 1 closes: /tmp is writable between the detach and the
    # switch, not only after it.
    machine.succeed("test 1777 = \"$(stat -c %a /tmp)\"")
    machine.succeed("touch /tmp/written-between-the-detach-and-the-switch")

    before = machine.succeed("readlink -f /run/current-system").strip()

    # Step 3: from `/`, not from wherever the caller happened to be.
    switch = "/run/current-system/specialisation/rootfsTmp/bin/switch-to-configuration"
    rc, out = machine.execute(f"cd / && {switch} switch 2>&1")
    print(f"=== switch output (exit {rc}) ===")
    print(out)

    assert rc == 0, f"the migration recipe did not get the switch to exit 0 (ENG-11080)\n{out}"

    after = machine.succeed("readlink -f /run/current-system").strip()
    assert after != before, f"switch returned 0 but /run/current-system is still {before}"

    machine.succeed("systemctl is-active dbus-broker.service")
    machine.succeed("busctl --system --no-pager status >/dev/null")
    machine.succeed("test 1777 = \"$(stat -c %a /tmp)\"")
    machine.succeed("touch /tmp/written-after-the-switch")
  '';
}
