/**
  Materialize `/ix` on a dev VM so a guest can bring up its own fleet from the
  same `dev.nix` it was built from (recursion).

  `/ix` holds the dev repo source. Two layouts:

  - On the shared volume (`onShare`): `/ix` is bound to `<mount>/ix`, so it is
    writable and edits propagate fleet-wide. The elected server seeds
    `<shareDir>/ix` once from the source (see `serverSeedModule`).
  - Standalone: each node gets its own writable copy under `/ix`, seeded once
    from the read-only store source via a systemd-tmpfiles `C` rule (copies
    only when the target is absent, so later edits survive reboots).

  The `ix` CLI itself is not yet shipped in dev images (only `claude-code` and
  `codex` are baked into `development-base`); putting it on `PATH` is the
  cross-repo follow-up RFC 0007 calls out. This module places the *source*; the
  CLI lands separately.

  Returns `{ nodeModule, serverSeedModule }`.
*/
{ lib }:
{
  /**
    Per-node `/ix` materialization.

    Arguments:
    - `src`: store path of the dev repo source (the flake `self`).
    - `onShare`: bind `/ix` to the shared volume instead of copying locally.
    - `mountPoint`: the shared volume mount point (used only when `onShare`).
  */
  nodeModule =
    {
      src,
      onShare ? false,
      mountPoint ? null,
    }:
    { ... }:
    if onShare then
      {
        fileSystems."/ix" = {
          device = "${mountPoint}/ix";
          fsType = "none";
          options = [
            "bind"
            "nofail"
            "x-systemd.requires-mounts-for=${mountPoint}"
          ];
        };
      }
    else
      {
        # Seed a writable working copy once; edits made inside the VM persist
        # because `C` only copies when `/ix` does not already exist.
        systemd.tmpfiles.rules = [
          "C /ix - - - - ${src}"
        ];
      };

  /**
    Seed `<shareDir>/ix` from the source on the elected server, once. Only used
    when `/ix` lives on the shared volume.
  */
  serverSeedModule =
    { src, shareDir }:
    { ... }:
    {
      systemd.tmpfiles.rules = [
        "C ${shareDir}/ix 0770 nobody nogroup - ${src}"
      ];
    };
}
