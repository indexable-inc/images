/**
  What lives on the shared volume: bound identity directories and `/ix`.

  Two concerns, one module since both are about node state that the SMB volume
  carries (RFC 0007):

  - `bindModule` binds `~/.claude` / `~/.n` onto the volume so the fleet shares
    one login. Only those two directories are shared, never the whole
    `~/.config`. The image's `/etc/claude-code/managed-settings.json` policy is
    untouched; only Claude's app-owned credential/state under `~/.claude` lives
    on the share, so the two layers do not collide.
  - `sourceNode` / `sourceServerSeed` materialize `/ix` (the dev source) for
    recursion: on the volume when one exists (writable, fleet-wide), else a
    local writable copy seeded once from the read-only store source.

  Returns module builders; `mkDev` chooses which to apply per node.
*/
{ lib }:
let
  # Bind-mount options for a path living under the SMB volume:
  # `requires-mounts-for` orders the bind after the CIFS mount so the source
  # exists first (the server pre-creates the subdir); `nofail` keeps boot moving
  # if the volume is still coming up.
  bindOptions = mountPoint: [
    "bind"
    "nofail"
    "x-systemd.requires-mounts-for=${mountPoint}"
  ];
in
{
  /**
    Bind identity dirs onto the volume.

    - `mountPoint`: where the SMB volume is mounted.
    - `binds`: list of `{ localPath, shareSubdir }`; each `localPath` (e.g.
      `/root/.claude`) is bound onto `<mountPoint>/<shareSubdir>`.
  */
  bindModule =
    { mountPoint, binds }:
    _: {
      fileSystems = lib.genAttrs' binds (
        bind:
        lib.nameValuePair bind.localPath {
          device = "${mountPoint}/${bind.shareSubdir}";
          fsType = "none";
          options = bindOptions mountPoint;
        }
      );
    };

  /**
    Per-node `/ix` materialization.

    - `src`: store path of the dev source (the flake `self`).
    - `onShare`: bind `/ix` to the volume instead of copying locally.
    - `mountPoint`: the volume mount point (used only when `onShare`).
  */
  sourceNode =
    {
      src,
      onShare ? false,
      mountPoint ? null,
    }:
    _:
    if onShare then
      {
        fileSystems."/ix" = {
          device = "${mountPoint}/ix";
          fsType = "none";
          options = bindOptions mountPoint;
        };
      }
    else
      {
        # Seed a writable working copy once; `C` only copies when `/ix` is
        # absent, so edits made inside the VM survive reboots.
        systemd.tmpfiles.rules = [ "C /ix - - - - ${src}" ];
      };

  /**
    Seed `<shareDir>/ix` from the source on the elected server, once. Used only
    when `/ix` lives on the volume.
  */
  sourceServerSeed =
    { src, shareDir }:
    _: {
      systemd.tmpfiles.rules = [ "C ${shareDir}/ix 0770 nobody nogroup - ${src}" ];
    };
}
