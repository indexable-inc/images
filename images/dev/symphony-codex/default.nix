{ ix, pkgs, ... }:
{
  ix.image = {
    name = "ix/symphony-codex";
    tag = "2026-05-28";
  };

  networking.hostName = "symphony-codex";

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
