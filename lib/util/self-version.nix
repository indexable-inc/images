/**
Resolve the building flake's own commit identity for embedding in build
artifacts (e.g. a CLI `--version` string). `dirtyRev`/`rev` come from the git
flake input; the date is reshaped from `lastModifiedDate`, which Nix sets to the
locked revision's committer timestamp (or, in a dirty tree, the newest
working-file mtime).

A flake `self` with no revision (a non-git `path`/tarball source, or an eval
that perturbs `self` such as `--override-input`) reports the whole identity as
"unknown" so consumers never embed an empty string. When a revision IS present
the flake always carries the committer timestamp too, so a `lastModifiedDate`
that is not the expected 14-digit `YYYYMMDDHHMMSS` shape is a broken invariant
and throws rather than emitting a bogus version.

Returns:
  commit     — full revision sha (no `-dirty` suffix), or "unknown".
  commitShort — first 10 chars of `commit`, or "unknown".
  commitDate — committer timestamp as second-precision UTC ISO 8601
               (`YYYY-MM-DDTHH:MM:SSZ`), or "unknown" when no revision is present.
  dirty      — whether the working tree was dirty at build time.
  version    — display string, e.g. `2026-06-07T08:10:28Z (51a9c880d6)` or
               `2026-06-07T08:10:28Z (51a9c880d6-dirty)`, or "unknown".
*/
{
  lib,
  self,
}: let
  rawRev = self.dirtyRev or self.rev or null;
in
  if rawRev == null
  then {
    commit = "unknown";
    commitShort = "unknown";
    commitDate = "unknown";
    dirty = false;
    version = "unknown";
  }
  else let
    dirty = lib.hasSuffix "-dirty" rawRev;
    commit = lib.removeSuffix "-dirty" rawRev;
    commitShort = builtins.substring 0 10 commit;
    # A revision is present, so the flake must also carry the committer
    # timestamp, formatted UTC as `YYYYMMDDHHMMSS` (exactly 14 digits). Reshape
    # it to second-precision ISO 8601 (still UTC); any other width means the
    # format assumption broke, so fail loudly rather than slice wrong offsets.
    rawDate = self.lastModifiedDate;
    commitDate =
      if builtins.stringLength rawDate == 14
      then "${builtins.substring 0 4 rawDate}-${builtins.substring 4 2 rawDate}-${builtins.substring 6 2 rawDate}T${builtins.substring 8 2 rawDate}:${
        builtins.substring 10 2 rawDate
      }:${builtins.substring 12 2 rawDate}Z"
      else throw "self-version.nix: expected self.lastModifiedDate to be 14 digits (YYYYMMDDHHMMSS) for revision ${commit}, got ${builtins.toJSON rawDate}";
    version = "${commitDate} (${commitShort}${lib.optionalString dirty "-dirty"})";
  in {
    inherit
      commit
      commitShort
      commitDate
      dirty
      version
      ;
  }
