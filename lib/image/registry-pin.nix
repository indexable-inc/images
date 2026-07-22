/**
Build the `nix.registry.index.to` value for a flake `self`: a `path`-type
pin on `self.outPath`, locked with `self.narHash` when `self` carries one.

`narHash` is conditional because `self` arrives in two shapes (index#3981,
shape landed in #3988):

- index's own flake, and images published by its CI (consumed via git):
  `self` carries `narHash` and the pin must keep it. An unlocked `path:` pin
  makes nix treat the input as mutable and re-hash AND re-copy the whole
  source tree on every in-guest eval (the #1748-era cost).
- a path-locked flake input (ix's `./index` submodule seam, ix#8142):
  path-type lock nodes carry no `narHash` — the parent flake's rev locks the
  content transitively — so `inherit (self) narHash` fails eval, and
  recomputing one via fetchTree is rejected in pure eval (the source is a
  lazy-tree subpath). The pin must simply omit `narHash`.

Extracted from `lib/image/default.nix` (its only production consumer) so the
`image-registry-pin` check can exercise both shapes with mock selves.
*/
{lib}: self:
{
  type = "path";
  path = self.outPath;
}
// lib.optionalAttrs (self ? narHash) {inherit (self) narHash;}
