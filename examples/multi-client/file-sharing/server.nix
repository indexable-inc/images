_: let
  sambaPort = 445;
  shareDir = "/var/lib/file-share";
in {
  # Userspace `smbd` rather than in-kernel `ksmbd`: ix images are
  # `boot.isContainer = true` and share the host `linux-ix` kernel, so
  # the SMB server has to live in userspace. Linux clients still use
  # `cifs.ko` from the host kernel.
  services.samba = {
    enable = true;
    # `ix.networking.expose.samba` below claims the port and opens the
    # in-guest firewall, so the listener policy stays in one place.
    openFirewall = false;

    settings = {
      global = {
        "workgroup" = "WORKGROUP";
        "server string" = "ix multi-client file share";
        "security" = "user";
        "map to guest" = "Bad User";
        "guest account" = "nobody";

        # SMB 3.1.1 on both ends. Older dialects predate the byte-range
        # lock improvements the brief is asking us to demonstrate.
        "server min protocol" = "SMB3_11";
        "client min protocol" = "SMB3_11";

        # Mediate every byte-range lock in `smbd` instead of pushing them
        # down to the host kernel's local-fs locks. Two CIFS clients
        # otherwise contend through the server's POSIX lock manager,
        # which Linux maps onto fcntl byte-range locks only — `flock()`
        # over CIFS would lose coordination across clients.
        "locking" = "yes";
        "strict locking" = "yes";
        "posix locking" = "yes";
        "oplocks" = "yes";
        "kernel oplocks" = "no";

        # Cross-client visibility over throughput: a write from
        # `client-0` should be readable from `client-1` without waiting
        # for cache to expire on its own.
        "strict sync" = "yes";
      };

      share = {
        "path" = shareDir;
        "browseable" = "yes";
        "read only" = "no";
        "writable" = "yes";
        "guest ok" = "yes";
        "force user" = "nobody";
        "force group" = "nogroup";
        "create mask" = "0660";
        "directory mask" = "0770";
      };
    };
  };

  systemd.tmpfiles.rules = [
    "d ${shareDir} 0770 nobody nogroup -"
  ];

  # One declaration registers the claim and opens the firewall for SMB.
  ix.networking.expose.samba = {
    port = sambaPort;
    description = "Samba SMB3 share for east-west clients";
  };

  ix.healthChecks.smbd = {
    description = "smbd is active";
    unit = "samba-smbd";
  };
}
