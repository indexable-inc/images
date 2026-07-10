# NixOS VM boot smoke test for the Minestom spleef example server.
#
# The spleef package's build-time guarantees stop at "the fat jar compiles and
# links against the pinned Minestom": nothing proves the jar actually boots and
# serves the Minecraft protocol under the `services.minestom` module it is
# documented to run under (doc/minestom/overview.md). This test closes that
# gap: it boots a NixOS VM with `services.minestom.serverJar` pointed at the
# spleef jar and asserts (1) the unit comes up, (2) Main logged its readiness
# line, (3) the port is open, and (4) a real Minecraft server-list ping
# answers with the pinned protocol version — twice, through both renderings
# of the shared Rust `mc-protocol` crate: `mc-probe` (Python over the pyo3
# unibind bindings, packages/minecraft/minecraft/probe, the same tool the
# minecraft/velocity modules use for health checks) and `mc-probe-kt`
# (Kotlin over the FFM/JVM unibind bindings,
# packages/minecraft/minecraft/probe-kt), so the e2e path exercises every
# binding surface against a live server.
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
  packages = ix.packageSetFor pkgs;
  spleefJar = packages.minestom.spleefServerJar;
  mcProbe = lib.getExe packages.mc-probe;
  mcProbeKt = lib.getExe packages.mc-probe-kt;
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

      # End-to-end protocol proof: a real server-list ping answered with a
      # well-formed status. Protocol 775 = Minecraft 26.1.2, in lockstep with
      # the Minestom pin in servers/spleef/build.gradle.kts; a version bump
      # there moves these assertions too. Both probes run so both unibind
      # renderings of mc-protocol (Python/pyo3, Kotlin/FFM) are proven
      # against a live server, not just their conformance fixtures.
      server.succeed("${mcProbe} 127.0.0.1:25565 --protocol-version 775 --timeout 30")
      server.succeed("${mcProbeKt} 127.0.0.1:25565 --protocol-version 775 --timeout 30")

      server.shutdown()
    '';
  }
