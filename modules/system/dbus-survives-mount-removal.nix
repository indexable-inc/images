# The system bus must not be collateral damage of a mount going away.
#
# nixpkgs gives dbus-broker `RequiresMountsFor = [ "/tmp" ]`
# (nixos/modules/services/system/dbus.nix), under the comment "We get errors
# when reloading the dbus-broker service if /tmp got remounted after this
# service started". Ordering is all that comment asks for, but
# `RequiresMountsFor=` is `Requires=` plus `After=`, and the `Requires=` half
# tells systemd to stop the bus whenever it stops the mount.
#
# That turns a routine config change into a broken machine. `dac64977` moved
# guest /tmp off a tmpfs, which is exactly such a change:
# switch-to-configuration stops the tmp.mount the new generation no longer
# declares, systemd stops the bus in the same transaction, and
# switch-to-configuration -- which is issuing those jobs over that bus and is
# blocked waiting for them -- loses its connection and exits 1.
# `/run/current-system` never advances and the activation phase never runs, so
# the rootfs /tmp the unmount exposed is left at mode 0555 with nothing coming
# to fix it. Three hyperion guests were repaired by hand (ENG-11080).
#
# Dropping to `WantsMountsFor=` keeps the ordering and drops only the teardown
# propagation. Measured on the unit systemd actually loaded, the ordering was
# never at risk: `PrivateTmp=` already contributes `WantsMountsFor=/tmp
# /var/tmp` to this same unit, so `After=tmp.mount` is there twice over and
# the nixpkgs line was contributing nothing but the kill switch. It is
# restated below rather than left to `PrivateTmp=`, so that turning that off
# some day does not silently take the ordering with it.
#
# Its own module rather than a few lines in `lib/image/platform.nix` because
# the test that proves it has to import the same code: platform.nix bakes
# OCI-image policy like `boot.isContainer` and cannot be instantiated in a
# test VM, so a fix living there could only be guarded by a copy of itself.
# tests/switch-stops-a-mount-vm.nix imports this file and fails without it.
#
# Upstream nixpkgs should carry this; a patch has not been submitted yet.
# Until it lands, the `WantsMountsFor` assertion in tests/default.nix is what
# notices if upstream changes shape underneath the override.
{lib, ...}: {
  systemd.services.dbus-broker.unitConfig = {
    # astlog-ignore: no-mkforce nixpkgs defines this as a list, and a second definition can only append to it; ENG-11080
    RequiresMountsFor = lib.mkForce [];
    WantsMountsFor = ["/tmp"];
  };
}
