# Login-shell setup for ix VMs. Sourced only for login shells (SSH
# session, console getty, `su -`), not for non-interactive command
# execution like `ssh root@vm -- whatever`.
#
# Enters the workspace the base profile pre-creates with
# systemd-tmpfiles. IX_WORKDIR is set by env.nu (generated from the
# base profile's shellWorkspace.directory option); the path-exists
# guard keeps this safe when shellWorkspace.enable is off and the
# directory was never created.
let target = $env.IX_WORKDIR?
if $target != null and ($target | path exists) {
  cd $target
}
