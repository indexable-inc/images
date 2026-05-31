{ ix, pkgs, ... }:
{
  ix.image = {
    name = "ix/symphony-codex";
    tag = "2026-05-31";
  };

  networking.hostName = "symphony-codex";

  # Symphony clones each run's repo checkouts under /workspace (Symphony's
  # Codex.Provision @ix_workspace_root = "/workspace/symphony"). The VM's
  # writable root is virtiofs-from-CAS served by the host vmfsd, whose
  # small-file write path caps at ~5-6k files/s and collapses to single
  # digits under host load, so a full-tree `git checkout` of the ~9k-file ix
  # repo onto the root never finishes inside the provision timeout and the
  # run falls back to host placement (ENG-2007, ENG-2004). Back /workspace
  # with tmpfs: the checkout writes into guest RAM (~36k files/s, immune to
  # host vmfsd contention) and the node-agent virtio-mem autoscaler grows
  # guest RAM on demand to cover it. size= is a ceiling, not a reservation
  # (pages are allocated lazily on write); 32 GiB comfortably fits the
  # ~2.5 GiB of checkouts plus a scoped `cargo check` target/ while bounding
  # a runaway run far below the VM's addressable memory cap.
  fileSystems."/workspace" = {
    device = "tmpfs";
    fsType = "tmpfs";
    options = [
      "size=32g"
      "mode=0755"
    ];
  };

  environment.systemPackages = [
    pkgs.ast-grep
    pkgs.bashInteractive
    pkgs.cacert
    pkgs.cmake
    pkgs.codex
    pkgs.coreutils
    pkgs.curl
    pkgs.direnv
    pkgs.fd
    pkgs.findutils
    pkgs.gcc
    pkgs.gh
    pkgs.git
    pkgs.gnugrep
    pkgs.gnumake
    pkgs.gnused
    pkgs.gnutar
    pkgs.gzip
    pkgs.jq
    pkgs.nodejs_24
    pkgs.openssh
    pkgs.pkg-config
    pkgs.python3
    pkgs.ripgrep
    pkgs.symphony-room-server
    pkgs.unzip
    pkgs.which
    pkgs.zstd
  ];

  networking.firewall = {
    allowedTCPPorts = [ 8080 ];
    allowedUDPPorts = [ 4433 ];
  };

  ix.networking.portClaims = {
    symphony-room-http = {
      protocol = "tcp";
      port = 8080;
      address = "0.0.0.0";
      description = "Symphony room-server HTTP";
    };

    symphony-room-webtransport = {
      protocol = "udp";
      port = 4433;
      address = "0.0.0.0";
      description = "Symphony room-server WebTransport";
    };
  };
}
