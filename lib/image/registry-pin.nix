/**
Build the `nix.registry.index.to` value for a flake `self`: a `path`-type pin
naming a TOP-LEVEL store path that holds index's source, locked with
`self.narHash` only when that store path is `self`'s own.

Why top-level and not just any path holding the source: nix's
`PathInputScheme::getAccessor` (nix `src/libfetchers/path.cc`) skips re-dumping
the tree only when `maybeParseStorePath` succeeds on the pinned path AND the
resulting store path is named `source` AND the store considers it valid.
Miss any one and it falls through to `addToStoreFromDump`, which through a
guest's VCFS store is the multi-minute first-`nix`-command cost this pin exists
to remove (index #1748/#1815). `base-image-nix-db` covers the validity leg; the
other two are this file's job.

`self` arrives in three shapes:

- index consumed as its own flake, by its own CI and by every
  `github:indexable-inc/index` consumer: `outPath` is a `-source` store path
  and `narHash` describes exactly it. Pin both, and copy nothing.
- index as ix's relative-path input, `index.url = "path:./index"` since ix#9290
  de-submoduled it: `outPath` is `<ix-source>/index`, a SUBPATH of ix's whole
  source tree, and there is no `narHash` at all (path-type lock nodes carry
  none, index#3981).
- index built through the enclosing repo: `nix build ./index#...` inside an ix
  checkout resolves to `git+file://...?dir=index`, so `outPath` is that same
  subpath while `narHash` IS present and describes the WHOLE ix tree.

The last two both need `sourceRoot` copied into a store path of its own, and
both must drop `narHash`: the second has none, and the third's would be a lock
on content the pin does not name.

Its own file, separate from `lib/image/default.nix` (its only production
consumer), so the `image-registry-pin` check can drive every branch with mock
selves.
*/
{lib}: {
  self,
  /**
  index's flake root as a path, copied into a store path of its own when
  `self.outPath` is not already one nix can short-circuit on. Forced only on
  that branch, which the check pins by passing a `throw` on the other.
  */
  sourceRoot,
}: let
  # A store path directly under `/nix/store` whose name is exactly `source`:
  # 32 characters of base-32 digest and nothing else. `builtins.match` anchors
  # the whole string, so a subpath fails on the `/` its remainder carries and
  # any other name fails on the digest run. Loosening the digest to `[^/]+`
  # looks equivalent and is not: it also admits `<digest>-index-source`, a name
  # PathInputScheme rejects just as flatly as it rejects a subpath.
  pinnable = builtins.match "${builtins.storeDir}/[a-z0-9]{32}-source" self.outPath != null;
  path =
    if pinnable
    then self.outPath
    # Interpolated, not `toString`: the string has to keep its context so the
    # copy roots into the image closure and `includeNixDB` registers it valid.
    # `name = "source"` is load-bearing, not cosmetic; see the header.
    else "${builtins.path {
      path = sourceRoot;
      name = "source";
    }}";
in
  {
    type = "path";
    inherit path;
  }
  // lib.optionalAttrs (pinnable && self ? narHash) {inherit (self) narHash;}
