{
  ix,
  callPackage,
  symlinkJoin,
  makeWrapper,
  # buildIxRustTool (the overlay build path) passes explicit `pkgs`/`ix`; absorb
  # the extras it injects so callPackage in both contexts is happy.
  ...
}: let
  bin = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "minecraft-sound";
    meta.mainProgram = "minecraft-sound";
  };

  # Mojang sound assets fetched at build time. See sounds.nix for the
  # licensing / do-not-upload constraints on this store path.
  sounds = callPackage ./sounds.nix {};
in
  symlinkJoin {
    name = "minecraft-sound";
    paths = [bin];
    nativeBuildInputs = [makeWrapper];

    # Bake the fetched sound pack in so the tool plays sounds with zero config and
    # no Minecraft install. `--set-default` keeps MCSOUND_ASSETS overridable at
    # runtime (e.g. to point at a real Minecraft / Prism install instead).
    postBuild = ''
      # shell
      wrapProgram $out/bin/minecraft-sound \
        --set-default MCSOUND_ASSETS ${sounds}/sounds
    '';

    inherit (bin) meta;
    # Drop `unchecked` so the overlay build path (`buildIxRustTool`, which returns
    # `passthru.unchecked` when present) yields this wrapped derivation rather than
    # unwrapping back to the bare, sound-pack-less binary.
    passthru = builtins.removeAttrs bin.passthru ["unchecked"];
  }
