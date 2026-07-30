# /tmp must be writable by everyone, and on an ix guest nothing else
# guarantees it.
#
# The directory in the image root is mode 0555. That is invisible while
# anything is mounted over it, and ix's injected PID 1 always mounts a tmpfs
# there before systemd exists
# (`crates/vm/guest/remote-bootstrap/src/main.rs`, `ESSENTIAL_MOUNTS`), so the
# 0555 underneath is only ever exposed at the moment that mount goes away.
#
# Which is exactly what retiring a generation-declared `tmp.mount` does.
# Measured on hyperion-game (2026-07-30 01:04): a switch whose entire stop list
# was `tmp.mount` completed cleanly -- bus alive, generation recorded, no failed
# units, which is ENG-11080's fix working -- and left
#
#     /tmp  dr-xr-xr-x  root root
#
# with the game server running as a `DynamicUser`. Root does not notice; every
# workload does. It is the same end state as ENG-11080's original failure,
# reached by a different road, and it had to be repaired by hand.
#
# WHY NOT A TMPFILES RULE. systemd ships `q /tmp 1777 root root 10d` already and
# it does not cover this: that rule lives in `systemd-tmpfiles-setup.service`,
# which reruns on a switch only when its configuration changed, and a switch
# that merely drops a mount unit does not change it. The switch above started
# `mandb.service` and nothing else. An unconditional activation snippet has no
# such dependency -- activation runs at boot AND on every switch -- so it fires
# exactly when the exposure can happen.
#
# WHY NOT FIX THE IMAGE ROOT. That is the cause, and it should also be fixed,
# but an image change reaches a guest only on a recreate. A guest that has
# already booted a bad root needs healing on its next switch, which is what this
# does. If the root is fixed later this snippet becomes a no-op that prints
# nothing, which is the right way for it to retire.
#
# Its own module, not a few lines in `lib/image/platform.nix`, so that
# `tests/switch-stops-a-mount-vm.nix` can import the same code it is proving.
# platform.nix bakes OCI-image policy like `boot.isContainer` and cannot be
# instantiated in a test VM, so a fix living there could only be guarded by a
# copy of itself.
_: {
  system.activationScripts.tmpWritable = ''
    mode=$(stat -c %a /tmp)
    if [ "$mode" != "1777" ]; then
      echo "platform: /tmp is $mode, restoring 1777"
      chmod 1777 /tmp
    fi
  '';
}
