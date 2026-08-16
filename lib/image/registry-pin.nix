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
on content the pin does not name. Those two shapes are also the only ones whose
copy is filtered; see the comment on `excludedTracked`.

Its own file, separate from `lib/image/default.nix` (its only production
consumer), so the `image-registry-pin` check can drive every branch with mock
selves. It returns `{ excludedTracked, pin }` rather than the `pin` function
alone so that check reads the exclusion list from here instead of restating it.
*/
{lib}: let
  /**
  Top-level entries of index's tree that the copied pin drops. Every one is
  git-tracked, which is why `image-registry-pin` can assert each still exists.

  WHAT THE UNFILTERED PIN COST (ENG-12789; audit evidence and drv diffs on the
  ticket). The pin's store path is a content hash of whatever it names, and it
  named all 72,809 files of `index/`, 841 MB of narSize. So a commit touching
  any file under `index/` moved it, which moved `etc-nix-registry.json`, which
  moved `etc`, `activate` and `nixos-system-<host>` for all 21 fleet hosts: 63
  derivations per commit, measured. Gate wall-clock was about 60 s, so the cost
  was never CPU. It was meaning: every fleet system closure was permanently
  dirty, a deploy could never say "nothing changed", and every fingerprint
  watcher downstream of a system closure fired on noise. Over 2,000
  index-touching commits, 444 of them (22%) touched nothing outside the list
  below, so about one cascade in five was pure churn. `views` alone is 67,261
  of those 72,809 files and 95% of the bytes, and the pin's string context
  roots it into every base image, so dropping it also takes the vendored
  upstream trees out of every guest closure.

  WHAT THE FILTER COSTS, AND WHO PAYS. This list is a maintained boundary: it
  has to keep tracking what index's flake actually reads, and nothing keeps it
  honest on its own. The two ways it drifts fail very differently, and both are
  slow, so each names its maintainer action.

  - TOO BROAD (an entry here starts being read by a flake output, or one of
    these directories is renamed and the rule goes dead). Fails SILENTLY. A
    dead rule restores the churn it was removing and nobody notices for weeks.
    The rename half is guarded: `image-registry-pin` asserts every name here
    still exists at the root of the real tree, and fires at eval the moment one
    is renamed away, whereupon re-point or delete the rule in the same commit
    as the rename. The half no assertion can catch is a NEW reader of an
    excluded directory, so whoever teaches a flake output to read `views/`,
    `doc/` or `playbook/` owes this list a second look.
  - TOO NARROW (something the flake reads gets excluded). Fails LOUDLY, but
    only where the filtered tree is actually evaluated, which is IN A GUEST
    against `index#<attr>`, possibly long after the commit that changed the
    read set. Our own gates evaluate index from the UNFILTERED source and
    cannot see it. `image-registry-pin` asserts the filtered copy still holds
    the roots the flake reads at eval time (`flake.nix`, `lib`, `packages`,
    ...), which covers the coarse case; the action on a real report from a
    guest is to delete the offending entry from this list, not to special-case
    the reader.

  The known, accepted narrowing: `nix run index#nix-ix` (and `#ghostty`, `#jj`,
  `#clippy`, ... every package built from a `views/` tree, see `viewSource` in
  lib/default.nix) no longer evaluates in a guest, because the source it names
  is not in the pin. Deliberate: a sandbox VM compiling a vendored nix or mesa
  checkout from source is not a workflow anyone has, and the base image already
  ships the tools those packages build.

  ALTERNATIVES REJECTED, recorded so nobody re-derives them. (1) DELETE THE PIN,
  the cheapest fix if nothing reads it. Rejected on evidence: no fleet host
  reads it (all 12 `nixosConfigurations` evaluate to a `nix.registry` holding
  `nixpkgs` and nothing else; the pin is set here, in the GUEST image config,
  and reaches host closures only because the regional-status units push that
  image), but every ix sandbox VM does. `index` is not in the upstream flake
  registry either -- 46 global rows, none of them `index` -- so dropping this
  entry does not degrade an in-guest `nix run index#<pkg>` to a network fetch,
  it fails the resolution outright, and index #1748/#1815 measured this exact
  path at 2m44.8s cold against ~2.6s pinned. (2) Pin by REV
  STRING instead of by path, a `github:indexable-inc/index/<rev>` entry.
  Rejected: the rev moves on every commit whatever that commit touched, so it
  buys none of the invalidation stability that is the entire point here, and it
  puts network resolution back on the first in-guest `nix` command, which is
  the cost index #1748/#1815 removed. (3) STABLE INDIRECTION: point the
  registry at a fixed profile path so the system closure stops embedding the
  source hash at all. That makes this failure class impossible rather than
  merely smaller, and it is the escalation if filtered churn still hurts.
  Deliberately not taken now: it costs closure purity (the registry names a
  mutable path the closure does not pin) and admits host skew (two hosts on one
  generation resolving `index` differently), a materially bigger design change
  than this ticket justifies.
  */
  excludedTracked = [
    "views"
    "doc"
    "playbook"
    ".claude"
    ".editorconfig"
    ".gitattributes"
    ".github"
    ".gitignore"
    ".memories"
    ".vscode"
    ".zed"
  ];

  /**
  Working-tree residue that a git-derived source never carries and a `path:`
  copy of a live checkout always does. Kept apart from `excludedTracked`
  precisely because it must NOT be asserted present: `target/` alone is 279 MB
  of build output in a working checkout and absent in CI.
  */
  excludedUntracked = [
    ".direnv"
    ".git"
    ".jj"
    "result"
    "target"
  ];

  # Anchored at the tree root by construction: the predicate reads only the
  # FIRST path segment relative to `root`, so the `views` rule cannot also
  # match a `packages/*/views`. Same shape as the manifest slice in
  # lib/rust/cargo-unit.nix.
  excludesTopLevel = root: let
    prefix = toString root + "/";
    excluded = excludedTracked ++ excludedUntracked;
  in
    path: _type:
      !(builtins.elem (lib.head (lib.splitString "/" (lib.removePrefix prefix path))) excluded);

  pin = {
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
      # Pinned as it stands, and deliberately UNFILTERED. Filtering here means
      # copying, which costs both the no-copy short-circuit and the `narHash`
      # lock this branch exists for, and it would buy nothing: this is index
      # consumed as its own flake, where the pinned path is index's own source
      # and moves on every index commit by definition.
      then self.outPath
      # Interpolated, not `toString`: the string has to keep its context so the
      # copy roots into the image closure and `includeNixDB` registers it valid.
      # `name = "source"` is load-bearing, not cosmetic; see the header.
      else "${builtins.path {
        path = sourceRoot;
        name = "source";
        filter = excludesTopLevel sourceRoot;
      }}";
  in
    {
      type = "path";
      inherit path;
    }
    // lib.optionalAttrs (pinnable && self ? narHash) {inherit (self) narHash;};
in {
  inherit excludedTracked pin;
}
