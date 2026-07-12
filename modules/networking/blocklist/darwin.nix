# nix-darwin branch of the blocklist: macOS has no /etc/hosts module, so rewrite
# the whole file at activation (localhost block + the sinkhole lines).
#
# Gated on a non-empty list so importing the module without data is a no-op
# rather than clobbering the host's /etc/hosts. Trade-off: emptying the list
# stops managing the file, leaving the last write in place.
{
  lib,
  config,
  ...
}: {
  imports = [./common.nix];
  config = lib.mkIf (config.networking.blockedHosts != []) {
    # nix-darwin's networking activation may restore a migration backup over
    # /etc/hosts, so write the managed file after that step.
    system.activationScripts.postActivation.text = ''
      echo "Updating /etc/hosts..."
      cat > /etc/hosts << 'EOF'
      ##
      # Host Database
      ##
      127.0.0.1	localhost
      255.255.255.255	broadcasthost
      ::1             localhost

      # Blocked hosts (networking.blockedHosts)
      ${config.networking.blockedHostsText}
      EOF
    '';
  };
}
