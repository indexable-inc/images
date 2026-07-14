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
{lib}: let
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
  checkEntry = pathStr: name: entry:
    if !(builtins.isAttrs entry)
    then throw "ix.lib.loadPins: ${pathStr}: pin `${name}` must be an object, got ${builtins.typeOf entry}"
    else if !(entry ? hash)
    then
      # Every pin needs the SRI fetch `hash`: fetchers pin by it, and an OCI
      # `imageDigest` pin still passes `hash` to dockerTools.pullImage. Requiring
      # it here fails with the owning path instead of deep inside the fetcher.
      throw "ix.lib.loadPins: ${pathStr}: pin `${name}` has no `hash` field"
    else if !(isSri entry.hash)
    then throw "ix.lib.loadPins: ${pathStr}: pin `${name}`.hash must be an `sha256-...` SRI string"
    else if (entry ? imageDigest) && !(isDigest entry.imageDigest)
    then throw "ix.lib.loadPins: ${pathStr}: pin `${name}`.imageDigest must be a `sha256:...` digest string"
    else if
      (entry ? prefetch)
      && !(lib.elem entry.prefetch [
        "file"
        "unpack"
        "manual"
      ])
    then
      # `prefetch` tells the generated updater how to recompute `hash` (see
      # mkUpdater below); reject typos at eval, not mid-update.
      throw "ix.lib.loadPins: ${pathStr}: pin `${name}`.prefetch must be `file`, `unpack`, or `manual`"
    else entry;

  /**
  Load and validate a sibling pins JSON, returning the parsed attrset of
  `{ <name> = { hash; ... }; }` for the caller to read fields from. `path` is
  normally a relative path literal (`./pins.json`) so the file joins the Nix
  import closure and a bad edit fails eval with the owning path named.
  */
  loadPins = path: let
    pathStr = toString path;
    data = lib.importJSON path;
  in
    if !(builtins.isAttrs data)
    then throw "ix.lib.loadPins: ${pathStr} must be a JSON object of `{ name = pin; }` entries"
    else lib.mapAttrs (checkEntry pathStr) data;

  /**
  Convenience for a single-pin file: load `path` and return the one named
  entry, throwing (with the file path) if the key is absent. Keeps
  single-pin call sites to `loadPin ./pins.json "src"` instead of
  `(loadPins ./pins.json).src`.
  */
  loadPin = path: name: let
    pins = loadPins path;
  in
    pins.${name} or (throw "ix.lib.loadPins: ${toString path} has no pin `${name}`");

  /**
  Build a `passthru.updateScript` that mechanically refreshes the SRI `hash`
  of every pin in a `pins.json`, so the pin joins `nix run .#update`.

  For each entry carrying a `url`, the script prefetches the URL and rewrites
  that entry's `hash` in place, preserving every other field (url, version,
  imageDigest, ...). It does NOT invent a new version or URL: bumping the
  upstream version is a human edit to `pins.json` (change url/version), after
  which the updater re-pins the hash — the same "human reviews the bump, the
  updater re-pins bytes" posture the yc CLI uses (no signed upstream manifest,
  so no provenance check; the CI build only proves the pinned bytes fetch).

  The hash a fetcher validates depends on the fetcher, so each pin declares
  how to recompute it via an optional `prefetch` field:

  - `"file"` (default): flat file hash via `nix store prefetch-file`. Correct
    for `fetchurl`-consumed pins.
  - `"unpack"`: unpacked-tree hash via `nix-prefetch-url --unpack`, which
    matches `fetchzip` with its default `stripRoot = true` (and `fetchCrate`,
    which unpacks the same way). Verified byte-identical against the existing
    vector-bin and wasm-bindgen-cli pins.
  - `"manual"`: the script never rewrites this hash. For pins no prefetch
    command reproduces — `fetchzip { stripRoot = false; }` post-processes the
    tree, so neither flat nor `--unpack` hashing matches; refresh by building
    the package with the new url and copying the `got:` hash from the
    mismatch error. Writing a best-guess hash here would brick the pin, which
    is worse than asking the human.

  Arguments:
  - `pinUpdate`: the repo's pin-update engine package
    (`packages/nix/pin-update`), whose `pins` mode implements this contract.
  - `nix`: the nix package (for `nix store prefetch-file`, `nix-prefetch-url`
    and `nix hash convert`).
  - `pname`: package name, for the script name and messages.
  - `relPath`: the pins.json path RELATIVE to the repo root (the updater runs
    from there, as the generated `update` app guarantees), e.g.
    `packages/vector-bin/pins.json`.

  An `imageDigest` pin is NOT auto-refreshed (its digest and hash both change
  only on a deliberate base-image bump, which is a human edit); the script
  leaves such entries untouched and reports them.
  */
  mkUpdater = {
    pinUpdate,
    nix,
    pname,
    relPath,
  }:
    pinUpdate.mkUpdateScript {
      name = "${pname}-update";
      description = "Re-pin the SRI hashes in ${relPath} from their pinned URLs";
      runtimeInputs = [nix];
      spec = {
        mode = "pins";
        pins = relPath;
      };
    };

  # Overlay consumers do not receive update tooling. Keep that nullable
  # package boundary here so every pinned package cannot drift into its own
  # copy of the same conditional wrapper.
  mkOptionalUpdater = args @ {pinUpdate, ...}:
    if pinUpdate == null
    then null
    else mkUpdater args;
in {
  inherit loadPins loadPin mkUpdater mkOptionalUpdater;
}
