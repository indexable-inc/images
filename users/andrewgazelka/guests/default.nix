# Personal vmkit macOS guests: data only. The machinery (plist rendering,
# ssh push, launchd bootstrap, brew install, the BEAM node) is
# modules/home/macos-guests.nix, imported alongside this module by
# profiles/darwin-home.nix.
#
# macos-primary is the agent's own mac (index#4360): its Messages is signed
# into the agent's Apple ID rather than the user's, so the agent has an
# identity that can be added to group chats. The sign-in, the Erlang cookie
# and the TCC grants are guest state, never rendered from nix; see README.md.
_: _: {
  macosGuests.macos-primary = {
    lifecycle.macAddress = "0e:c9:c7:6c:25:a8";
    ssh = {
      host = "192.168.64.6";
      user = "ix";
    };
    beamNode.enable = true;
  };
}
