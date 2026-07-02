# Read a package's pinned hashes/digests from a sibling `pins.json` file
# instead of inlining them in the `.nix`. This is the general counterpart of
# the Minecraft-only `lib/util/artifacts.nix` reader: one place that parses and
# typechecks a lock JSON so a routine bump touches one data file and never a
# `hash = "sha256-..."` literal in a tracked `.nix`.
#
# The JSON is the single source of truth an updater rewrites mechanically. Shape:
#
#   {
#     "<pin-name>": {
#       "hash": "sha256-...",      # required for a fetch pin, OR
#       "imageDigest": "sha256:...", "hash": "sha256-...",  # OCI image pin
#       "url": "...", "rev": "...", "version": "..."         # optional metadata
#     },
#     ...
#   }
#
# A pin entry carries its `hash` alongside the coordinates (url/rev/version)
# that produced it, so `nix run .#update` can refetch and overwrite the whole
# entry in one pass. Extra keys are allowed and ignored, so an updater may store
# whatever it needs (slug, platform map, ...).
{ lib }:
let
  isSri = h: lib.isString h && lib.hasPrefix "sha256-" h;
  # OCI digests are the `sha256:<hex>` form dockerTools.pullImage's imageDigest
  # takes, distinct from the SRI `sha256-` fetch hash it also wants.
  isDigest = d: lib.isString d && lib.hasPrefix "sha256:" d;

  /**
    Validate one pin entry against `path` (for error attribution) and `name`
    (its key). Every entry must carry at least one hash-shaped field: an SRI
    `hash` (fetchers) and/or an `imageDigest` (OCI pulls, which also carry an
    SRI `hash`). Coordinates and other keys pass through untouched.
  */
  checkEntry =
    pathStr: name: entry:
    if !(builtins.isAttrs entry) then
      throw "ix.lib.loadPins: ${pathStr}: pin `${name}` must be an object, got ${builtins.typeOf entry}"
    else if (entry ? hash) && !(isSri entry.hash) then
      throw "ix.lib.loadPins: ${pathStr}: pin `${name}`.hash must be an `sha256-...` SRI string"
    else if (entry ? imageDigest) && !(isDigest entry.imageDigest) then
      throw "ix.lib.loadPins: ${pathStr}: pin `${name}`.imageDigest must be a `sha256:...` digest string"
    else if !(entry ? hash) && !(entry ? imageDigest) then
      throw "ix.lib.loadPins: ${pathStr}: pin `${name}` has no `hash` or `imageDigest` field"
    else
      entry;

  /**
    Load and validate a sibling pins JSON, returning the parsed attrset of
    `{ <name> = { hash; ... }; }` for the caller to read fields from. `path` is
    normally a relative path literal (`./pins.json`) so the file joins the Nix
    import closure and a bad edit fails eval with the owning path named.
  */
  loadPins =
    path:
    let
      pathStr = toString path;
      data = lib.importJSON path;
    in
    if !(builtins.isAttrs data) then
      throw "ix.lib.loadPins: ${pathStr} must be a JSON object of `{ name = pin; }` entries"
    else
      lib.mapAttrs (checkEntry pathStr) data;

  /**
    Convenience for a single-pin file: load `path` and return the one named
    entry, throwing (with the file path) if the key is absent. Keeps
    single-pin call sites to `loadPin ./pins.json "src"` instead of
    `(loadPins ./pins.json).src`.
  */
  loadPin =
    path: name:
    let
      pins = loadPins path;
    in
    pins.${name} or (throw "ix.lib.loadPins: ${toString path} has no pin `${name}`");
in
{
  inherit loadPins loadPin;
}
