/**
  Bind shared identity directories onto the SMB volume.

  Gives a whole dev fleet one Claude login (and, optionally, one ix CLI
  identity) without running `claude login` / `ix login` on every node. The
  first node to authenticate writes its credentials to the share; every other
  node sees them immediately because the directory it reads from *is* the
  shared mount.

  Only `~/.claude` and `~/.n` are shared, never the whole `~/.config`: that
  keeps the blast radius to two credential stores on a single user's private
  fleet, the boundary `examples/synced-github-auth` argues for. The image's
  `/etc/claude-code/managed-settings.json` policy layer is untouched and stays
  in the image; only Claude's app-owned credential/state under `~/.claude`
  lives on the share, so the two layers do not collide.

  Returns a module builder. `mkDev` decides which binds are active from
  `shared.claudeAuth` / `shared.ixAuth`.
*/
{ lib }:
{
  /**
    Arguments:
    - `mountPoint`: where the SMB volume is mounted in the guest.
    - `binds`: list of `{ localPath, shareSubdir }`. Each `localPath` (e.g.
      `/root/.claude`) is bind-mounted onto `<mountPoint>/<shareSubdir>`.
  */
  authModule =
    { mountPoint, binds }:
    { ... }:
    {
      # Bind each identity dir onto its share subdir. `requires-mounts-for`
      # orders the bind after the CIFS mount so the source exists first; the
      # server pre-creates the subdir (see shared-mount `subdirs`). `nofail`
      # keeps boot moving if the volume is still coming up.
      fileSystems = lib.listToAttrs (
        map (
          bind:
          lib.nameValuePair bind.localPath {
            device = "${mountPoint}/${bind.shareSubdir}";
            fsType = "none";
            options = [
              "bind"
              "nofail"
              "x-systemd.requires-mounts-for=${mountPoint}"
            ];
          }
        ) binds
      );
    };
}
