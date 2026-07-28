# Ghostty terminal configuration.
#
# The config is a real Ghostty file, checked in at
# users/andrewgazelka/config/ghostty/config.ghostty, and the macOS config path
# is an out-of-store symlink to it in the checkout. Edit the file, hit
# `cmd+shift+r`, done — no switch, and no drift to lift back into Nix.
#
# Only one config path is written, so cumulative settings (custom-shader) are
# not double-applied. Themes and shaders stay under ~/.config/ghostty, linked
# from the shared modules/home/ghostty in profiles/workstation.nix.
{
  config,
  lib,
  ...
}: let
  cfg = config.users.andrewgazelka;
  target = "${cfg.paths.indexCheckout}/users/andrewgazelka/config/ghostty/config.ghostty";
  deployed = "Library/Application Support/com.mitchellh.ghostty/config";
in {
  home = {
    file."${deployed}".source = config.lib.file.mkOutOfStoreSymlink target;

    # The config used to be deployed by `mutable.files` as a plain writable
    # file, which Home Manager will not overwrite with a link. index-delta
    # forgets undeclared files but leaves them on disk, so drop the regular
    # file (never a link: an existing generation link is Home Manager's to
    # replace) before checkLinkTargets runs.
    activation.migrateGhosttyConfig = config.lib.dag.entryBefore ["checkLinkTargets"] ''
      configFile=${lib.escapeShellArg "${config.home.homeDirectory}/${deployed}"}
      if [[ -f "$configFile" ]] && [[ ! -L "$configFile" ]]; then
        run rm "$configFile"
      fi
    '';
  };
}
