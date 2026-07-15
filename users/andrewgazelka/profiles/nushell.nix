{configRoot}: {
  config,
  lib,
  ...
}: let
  sourceRoot = configRoot + "/nushell";
  checkoutRoot = "${config.users.andrewgazelka.paths.indexCheckout}/users/andrewgazelka/config/nushell";
  files = lib.filesystem.listFilesRecursive sourceRoot;
  relativePath = file:
    lib.removePrefix "${toString sourceRoot}/" (toString file);
in {
  # Nushell writes history and plugin state beside its config on macOS. Keep
  # that directory writable while linking every tracked leaf to the mutable
  # index checkout, so edits take effect without a Home Manager switch.
  home.file = lib.genAttrs' files (
    file: let
      path = relativePath file;
    in
      lib.nameValuePair "Library/Application Support/nushell/${path}" {
        source = config.lib.file.mkOutOfStoreSymlink "${checkoutRoot}/${path}";
      }
  );

  # Home Manager does not replace a managed directory symlink when its source
  # changes to leaf links. Remove only the previous generation-owned link so
  # linkGeneration can create the writable parent directory.
  home.activation.migrateNushellDataDirectory = config.lib.dag.entryBefore ["checkLinkTargets"] ''
    dataDir=${lib.escapeShellArg "${config.home.homeDirectory}/Library/Application Support/nushell"}
    if [[ -L "$dataDir" ]] && [[ $(readlink "$dataDir") == /nix/store/*-home-manager-files/* ]]; then
      run rm "$dataDir"
    fi
  '';
}
