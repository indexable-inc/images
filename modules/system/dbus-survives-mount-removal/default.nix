# The system bus must not be stopped when a mount is.
#
# nixpkgs' `nixos/modules/services/system/dbus.nix` sets
# `RequiresMountsFor = [ "/tmp" ]` on dbus-broker, under a comment asking for
# ordering: "We get errors when reloading the dbus-broker service if /tmp got
# remounted after this service started" (nixpkgs c4b3e4f5f89b, "makes sure that
# ... dbus-broker gets ordered after them"). `RequiresMountsFor=` is
# `Requires=` plus `After=`, and the `Requires=` half tells systemd to stop the
# bus whenever it stops the mount.
#
# `dac64977` moved guest /tmp off a tmpfs, so switch-to-configuration stops the
# tmp.mount the new generation no longer declares, systemd stops the bus in the
# same transaction, and switch-to-configuration -- issuing those jobs over that
# bus and blocked waiting for them -- loses its connection and exits 1 with
# `disconnected from D-Bus?`. `/run/current-system` never advances, activation
# never runs, and the rootfs /tmp the unmount exposed is left at mode 0555 with
# nothing coming to fix it. Three hyperion guests were repaired by hand, and a
# fourth reproduced it verbatim on 2026-07-29 (ENG-11080).
#
# `WantsMountsFor=` keeps the `After=` and drops the `Requires=`. That is three
# things dropped, not one: stop propagation, restart propagation, and the
# start-time gate that kept the bus from starting when the mount failed. The
# gate is the only loss worth arguing about, and it is acceptable because a
# post-`dac64977` guest has no tmp.mount to fail: there is nothing left to gate
# on. The ordering itself is not at risk -- systemd adds the `After=` edge for
# every mount-dependency type and takes only the Requires/Wants half from the
# type (`unit_add_mount_dependencies()`, src/core/unit.c).
#
# WHAT WAS CHECKED AND DID NOT HAPPEN. `PrivateTmp=true` in upstream's
# dbus-broker.service puts the unit's private directory under the very /tmp
# being retired, and systemd#28515 reports reloads failing `226/NAMESPACE`
# afterwards. Since a switch reloads the bus (nixpkgs puts the dbus config
# directory in `restartTriggers` and marks it `reloadIfChanged`), that would be
# the next nonzero exit in the queue, and `PrivateTmp = "disconnected"` would
# be the answer. It does not reproduce here: the upstream reproducer -- replace
# /tmp underneath the running bus, then reload it -- succeeds on systemd 261,
# measured in tests/switch-stops-a-mount-vm.nix, which is why that change is
# not in this module. The test keeps the reload, so if a future systemd or
# dbus-broker makes it real, it fails here rather than on a guest.
#
# WHAT THIS DOES NOT REACH. It only helps a machine that has already taken it.
# switch-to-configuration reads the RUNNING generation's unit files and reloads
# only after its stop phase, so the first switch onto this module still behaves
# like today's. ENG-11080 carries what a guest already running a tmpfs /tmp
# needs done to it first.
#
# Its own module rather than a few lines in `lib/image/platform.nix` because
# the test that proves it has to import the same code: platform.nix bakes
# OCI-image policy like `boot.isContainer` and cannot be instantiated in a test
# VM, so a fix living there could only be guarded by a copy of itself.
# tests/switch-stops-a-mount-vm.nix imports this file and fails without it.
#
# Upstream nixpkgs should carry this; no patch has been submitted yet. The
# `machine.fail` on `Requires=tmp.mount` in that test is what notices if
# upstream reshapes underneath the override -- the eval assertion in
# tests/default.nix cannot, because it reads the merged value this module sets.
{
  config,
  lib,
  ...
}:
# nixpkgs only defines this unit under the broker
# (`mkIf (cfg.implementation == "broker")`), and `services.dbus.enable`
# defaults off. Unguarded, this module would invent a standalone
# dbus-broker.service with a [Unit] section and no ExecStart on any host using
# the reference daemon -- no build error, just a module that looks like it does
# something.
lib.mkIf (config.services.dbus.enable && config.services.dbus.implementation == "broker") {
  systemd.services.dbus-broker.unitConfig = {
    # astlog-ignore: no-mkforce nixpkgs defines this as a list, and a second definition can only append to it; ENG-11080
    RequiresMountsFor = lib.mkForce [];
    WantsMountsFor = ["/tmp"];
  };
}
