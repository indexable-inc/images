{configRoot}: {
  config,
  lib,
  ...
}: let
  sourceRoot = configRoot + "/nushell";
  checkoutRoot = "${config.users.andrewgazelka.paths.indexCheckout}/users/andrewgazelka/config/nushell";
  rootEntries = builtins.readDir sourceRoot;
  entries = builtins.attrNames rootEntries;
  directories = builtins.filter (name: rootEntries."${name}" == "directory") entries;
in {
  # Nushell writes history and plugin state beside its config on macOS. Keep
  # that root writable while linking each tracked top-level entry to the
  # mutable checkout, so new files in config directories need no switch.
  home = {
    file = lib.genAttrs' entries (
      name:
        lib.nameValuePair "Library/Application Support/nushell/${name}" {
          source = config.lib.file.mkOutOfStoreSymlink "${checkoutRoot}/${name}";
        }
    );

    # Home Manager does not replace a managed directory symlink when its source
    # changes to leaf links. Remove only the previous generation-owned link so
    # linkGeneration can create the writable parent directory.
    activation.migrateNushellDataDirectory = config.lib.dag.entryBefore ["checkLinkTargets"] ''
      dataDir=${lib.escapeShellArg "${config.home.homeDirectory}/Library/Application Support/nushell"}
      if [[ -L "$dataDir" ]] && [[ $(readlink "$dataDir") == /nix/store/*-home-manager-files/* ]]; then
        run rm "$dataDir"
      fi
    '';

    # #3401: The previous layout created real directories containing
    # generation-owned leaf links. Refuse migration for unknown directories
    # or leaves that are not links from a Home Manager generation.
    activation.migrateNushellConfigDirectories = config.lib.dag.entryBefore ["checkLinkTargets"] ''
      dataDir=${lib.escapeShellArg "${config.home.homeDirectory}/Library/Application Support/nushell"}
      checkoutDir=${lib.escapeShellArg checkoutRoot}
      directories=(${lib.escapeShellArgs directories})
      isManagedDirectory() {
        local target="$1"
        local source="$2"
        local entry name tracked
        for entry in "$target"/*; do
          name="''${entry##*/}"
          tracked="$source/$name"
          if [[ -d "$entry" ]] && [[ ! -L "$entry" ]]; then
            if [[ ! -d "$tracked" ]] || ! isManagedDirectory "$entry" "$tracked"; then
              return 1
            fi
          elif [[ ! -L "$entry" ]] || [[ $(readlink "$entry") != /nix/store/*-home-manager-files/* ]]; then
            return 1
          fi
        done
        return 0
      }
      for directory in "''${directories[@]}"; do
        target="$dataDir/$directory"
        source="$checkoutDir/$directory"
        if [[ -d "$target" ]] && [[ ! -L "$target" ]]; then
          (
            shopt -s dotglob nullglob
            if isManagedDirectory "$target" "$source"; then
              run rm -r "$target"
            fi
          )
        fi
      done
    '';
  };
}
