{
  ix,
  pkgs,
}: let
  site = ix.buildSvelteSite pkgs {
    sourceRoot = ./site;
    serve.enable = false;
  };

  server = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "ix-term-server";
    meta.mainProgram = "ix-term-server";
  };
in
  # The Rust server and the Svelte site are built independently, then composed:
  # the site is installed as a resource and its path is handed to the server via
  # `IX_TERM_SITE_DIR` (the same shape as nix-web-monitor).
  ix.wrapPackage pkgs {
    package = server;
    resources.site = {
      source = site;
      from = "share/ix-term-site";
      to = "share/ix-term";
      env = "IX_TERM_SITE_DIR";
    };
    passthru = {
      tests =
        server.passthru.tests
        // {
          inherit site;
        };
      inherit site;
    };
    meta = {
      description = "Tailnet-internal web terminal on server-side libghostty-vt";
      mainProgram = "ix-term-server";
    };
  }
