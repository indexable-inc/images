# Personal vmkit macOS guests: data only. The machinery (plist rendering,
# ssh push, launchd bootstrap) is modules/home/macos-guests.nix, imported
# alongside this module by profiles/darwin-home.nix.
#
# macos-primary is the agent's own mac: signed into iMessage under the
# agent's Apple ID rather than the user's, so it has an identity of its own
# that can be added to group chats. It runs one resource, a BEAM node the
# host kernel calls into over distributed Erlang. The Messages sign-in, the
# contact card and the TCC grants are guest state, never rendered from nix
# (see README.md).
_: {pkgs, ...}: let
  # The node runs on the guest's own Elixir, at a path this module cannot
  # manage: nix has no store on the guest -- its installer cannot create
  # /etc/fstab there, even as root -- so the interpreter comes from
  # Homebrew and is named here as a plain guest path.
  guestElixir = "/opt/homebrew/bin/elixir";

  # A long name, not a short one: `-sname` needs both ends to share a DNS
  # suffix, which the vmnet bridge does not give us.
  nodeName = "ixagent@192.168.64.6";

  # Distribution picks an ephemeral port per node unless pinned, and the
  # host has to reach it across the bridge, so pin one and leave epmd on
  # its own 4369.
  distPort = 9100;

  # A store shebang would name a /nix path the guest does not have, so the
  # script is written with the guest's own bash.
  nodeScript = pkgs.writeTextFile {
    name = "ix-agent-node";
    executable = true;
    text = ''
      #!/bin/bash
      # launchd agents get the bare system PATH, and elixir execs `erl` off
      # PATH: without this the agent dies with "exec: erl: not found".
      export PATH="/opt/homebrew/bin:$PATH"
      exec ${guestElixir} \
        --erl "-kernel inet_dist_listen_min ${toString distPort} inet_dist_listen_max ${toString distPort}" \
        --name ${nodeName} \
        --cookie "$(cat "$HOME/.erlang.cookie")" \
        -e 'Process.sleep(:infinity)'
    '';
  };
in {
  macosGuests.macos-primary = {
    lifecycle.macAddress = "0e:c9:c7:6c:25:a8";
    ssh = {
      host = "192.168.64.6";
      user = "ix";
    };

    # The cookie the script reads is a shared secret and stays guest state:
    # a nix-rendered cookie would be world-readable in /nix/store.
    binaries.ix-agent-node.source = nodeScript;

    launchAgents."dev.ix.agent-node".config = {
      ProgramArguments = ["/Users/ix/.local/bin/ix-agent-node"];
      EnvironmentVariables = {
        HOME = "/Users/ix";
        PATH = "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin";
      };
      RunAtLoad = true;
      KeepAlive = true;
      ProcessType = "Background";
      # macOS privacy consent attaches to the process that runs, so the
      # grants this node needs (Full Disk Access to read Messages,
      # Automation to script it) follow the fixed path above and survive a
      # content update in place.
      StandardOutPath = "/tmp/ix-agent-node.log";
      StandardErrorPath = "/tmp/ix-agent-node.log";
    };
  };
}
