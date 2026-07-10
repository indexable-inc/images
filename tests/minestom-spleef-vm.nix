# NixOS VM boot smoke test for the Minestom spleef example server.
#
# The spleef package's build-time guarantees stop at "the fat jar compiles and
# links against the pinned Minestom": nothing proves the jar actually boots and
# serves the Minecraft protocol under the `services.minestom` module it is
# documented to run under (doc/minestom/overview.md). This test closes that
# gap: it boots a NixOS VM with `services.minestom.serverJar` pointed at the
# spleef jar and asserts (1) the unit comes up, (2) Main logged its readiness
# line, (3) the port is open, and (4) a real Minecraft server-list ping —
# handshake + status request over the wire protocol — gets a well-formed
# status JSON back.
#
# Minestom needs no bootstrap step (no paperclip, no EULA, no world download),
# so unlike tests/minecraft-blocks-vm.nix there is no build-time pre-patching:
# the jar is self-contained and the VM never wants the network.
{
  lib,
  pkgs,
  ix,
  paths,
}: let
  spleefJar = (ix.packageSetFor pkgs).minestom.spleefServerJar;
in
  pkgs.testers.runNixOSTest {
    name = "minestom-spleef-boot";

    # services/minestom reads the repo's cross-module helper bundle
    # (`ix.systemdHardening`, `ix.languages.java`) from its module args;
    # normally injected by `evalImageConfig`'s specialArgs.
    node.specialArgs.ix = ix;

    nodes.server = {...}: {
      imports = [
        (paths.modules + "/services/minestom")

        # The `ix.networking.portClaims` slot the module writes to is declared
        # by the image platform module (lib/image/platform.nix), which cannot
        # be imported here: it bakes OCI-image policy like `boot.isContainer`
        # that conflicts with the test VM. Declare just that option slot;
        # nothing reads it in this test, so a permissive type suffices.
        {
          options.ix.networking.portClaims = lib.mkOption {
            type = lib.types.attrsOf lib.types.anything;
            default = {};
          };
        }
      ];

      # Minestom is lightweight (no world gen at startup — the lobby platform
      # generates lazily per chunk), but the JVM heap tracks RAM via
      # MaxRAMPercentage, so give it modest headroom over the 1 GiB default.
      virtualisation = {
        memorySize = 2048;
        cores = 2;
      };

      services.minestom = {
        enable = true;
        serverJar = spleefJar;
      };
    };

    testScript = ''
      server.start()
      server.wait_for_unit("minestom.service")

      # Main's readiness line: MinecraftServer.start() returned, so the
      # network stack is bound and the lobby instance is registered.
      server.wait_until_succeeds(
          "journalctl -u minestom.service --grep 'spleef server listening on :25565' --quiet",
          timeout=300,
      )
      server.wait_for_open_port(25565)

      # End-to-end protocol proof: a real server-list ping (handshake +
      # status request, varint framing and all) answered with status JSON.
      server.succeed("${lib.getExe pkgs.python3} ${./minestom-spleef-vm/ping.py}")

      server.shutdown()
    '';
  }
