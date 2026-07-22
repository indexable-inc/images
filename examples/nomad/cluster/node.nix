/**
One nomad agent, either role.

Everything server and clients share: the nomad service, the datacenter, and
bind/advertise addressing. `settings` is the whole nomad config as typed Nix
values; the module renders the JSON nomad reads (nomad's config language is
HCL, and HCL's JSON form is a first-class input).
*/
_: {
  services.nomad = {
    enable = true;
    # Tasks run as plain processes from the node's own nix store (raw_exec,
    # see job.nix), so there is no container runtime to manage, and nomad
    # itself must stay root to supervise them.
    enableDocker = false;
    dropPrivileges = false;

    settings = {
      datacenter = "dc1";
      # Bind everywhere so the CLI and health checks work over loopback, but
      # advertise the east-west address: go-sockaddr's GetPrivateIP resolves
      # it at startup, which is nomad's own answer to "my IP only exists at
      # runtime".
      bind_addr = "0.0.0.0";
      advertise = {
        http = "{{ GetPrivateIP }}";
        rpc = "{{ GetPrivateIP }}";
        serf = "{{ GetPrivateIP }}";
      };
    };
  };

  ix.healthChecks.nomad-active = {
    description = "nomad agent is active";
    unit = "nomad";
  };
}
