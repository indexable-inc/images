_:
/**
Drop the `meta.license` marker on a vendored proprietary binary.

The per-system flake package set evaluates nixpkgs without `allowUnfree`, so
a wrapper around a binary tagged `licenses.unfree` (or one tagging its own
fetched binary that way) would block plain `nix run .#<name>`. Rather than
gating the output behind an `allowUnfree` toggle for software the repo has
already decided to ship, strip the marker at the wrapper boundary; the
vendor's commercial license still governs distribution, this only removes
the eval gate. One helper so each vendored-agent wrapper (claude-code,
cursor-cli, ...) reuses the workaround instead of restating this rationale.
*/
pkg:
pkg.overrideAttrs (previousAttrs: {
  meta = builtins.removeAttrs previousAttrs.meta ["license"];
})
