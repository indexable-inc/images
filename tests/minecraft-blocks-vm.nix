# NixOS VM boot smoke test for the minecraft-blocks Paper plugin (ENG-2186).
#
# The example's `eval` coverage is pure config-eval plus an offline ClickHouse
# integration check; the plugin itself is only guaranteed to *link* against the
# pinned Paper API. This test closes the remaining gap: it boots a NixOS VM
# running the same Paper server + BlockEvents plugin wiring as the example's
# producer node and asserts `onEnable` ran to completion (the plugin logged its
# success line and opened its JSON Lines log) with no enable-time exception in
# the journal.
#
# Why not boot `examples/minecraft/blocks` producer.nix verbatim: that module
# is a fleet member (it resolves `nodes.log` / `nodes.view` endpoints and ships
# telemetry), so it cannot evaluate as a standalone machine. The slice under
# test here is exactly the producer's `services.minecraft` block: same server
# jar, same plugin jar from `packages.nix`, same managed
# `plugins/BlockEvents/config.yml` handoff. The Kafka/OTel legs have their own
# eval assertions in tests/default.nix.
#
# Paper ships as a paperclip bootstrapper: on first launch it downloads
# Mojang's vanilla `server.jar` (URL + SHA-256 baked into the paperclip jar's
# `META-INF/download-context`), applies its binary patch, and extracts the
# runtime libraries. The VM test sandbox has no network, so that bootstrap is
# done at *build* time instead: `prePatched` seeds paperclip's `cache/` with
# the pinned vanilla jar (a fixed-output fetch) and runs paperclip with
# `-Dpaperclip.patchonly=true`. The VM then starts from the pre-bootstrapped
# `versions/` + `libraries/` trees, and paperclip's own hash checks accept
# them offline.
{
  lib,
  pkgs,
  ix,
  paths,
}: let
  packages = import (paths.examples + "/minecraft/blocks/packages.nix") {inherit ix pkgs;};

  # The producer pins this Minecraft version (examples/minecraft/blocks).
  version = "26.1.2";
  paperJar = ix.artifacts.minecraft.servers."${version}-paper";

  # Mojang's vanilla server jar for ${version}, pinned by URL + SRI hash like
  # every other artifact intake (lib/util/artifacts.nix). This must stay in
  # lockstep with the paper jar's `META-INF/download-context`; `prePatched`
  # verifies that at build time and fails with an update instruction if the
  # Paper build is bumped without this pin moving.
  vanilla = ix.artifacts.attachArtifactSources (
    lib.importJSON ./minecraft-blocks-vm/vanilla-server.json
  );

  # The default JRE the minecraft module launches the server with; the patch
  # step uses the same one so the class-file versions paperclip sees at build
  # time match what the VM runs.
  jvmVersion = ix.languages.java.defaultJvmVersion;
  jre = pkgs."temurin-jre-bin-${jvmVersion}";

  # Run the paperclip bootstrap offline at build time: seed the cache with the
  # pinned vanilla jar (named whatever `download-context` asks for), let
  # paperclip verify the hash and apply its patch, and keep the resulting
  # `versions/` + `libraries/` trees. Everything paperclip writes is itself
  # hash-verified against the jar's embedded manifests, so the output is
  # deterministic.
  prePatched =
    pkgs.runCommand "paper-${version}-prepatched"
    {
      nativeBuildInputs = [
        jre
        pkgs.unzip
      ];
    }
    ''
      mkdir -p work/cache
      cd work

      # download-context is one tab-separated line (no trailing newline, so
      # `read` hits EOF and returns nonzero even after filling the fields):
      # <sha256> <url> <cache file name>.
      unzip -p ${paperJar} META-INF/download-context > ctx
      IFS=$'	' read -r sha url fname < ctx || true
      if [ -z "$sha" ] || [ -z "$fname" ]; then
        echo "FAIL: could not parse META-INF/download-context from ${paperJar}" >&2
        exit 1
      fi

      cp ${vanilla.mojang-server.src} "cache/$fname"
      if ! echo "$sha  cache/$fname" | sha256sum --check --status -; then
        echo "FAIL: pinned vanilla server jar does not match this Paper build's download-context" >&2
        echo "  paperclip wants: $sha  $url" >&2
        echo "  update tests/minecraft-blocks-vm/vanilla-server.json to that URL + hash" >&2
        exit 1
      fi

      java -Dpaperclip.patchonly=true -jar ${paperJar}

      # Keep the cache too: at every startup paperclip re-verifies the
      # vanilla jar in cache/ (DownloadContext.download short-circuits only
      # when the file is present with a matching hash) before checking the
      # patched outputs, so an empty cache means a network fetch even with
      # valid versions/ + libraries/ trees.
      mkdir -p "$out"
      cp -r versions libraries cache "$out/"
    '';

  blockLog = "/var/lib/minecraft/block-events.jsonl";
in
  pkgs.testers.runNixOSTest {
    name = "minecraft-blocks-paper-boot";

    # The minecraft module tree reads the repo's cross-module helper bundle.
    # `ix.packages` is normally injected by `evalImageConfig`'s specialArgs; the
    # public lib surface carries `packageSetFor` instead, so rebuild it here.
    node.specialArgs.ix =
      ix
      // {
        packages = ix.packageSetFor pkgs;
      };

    nodes.producer = {...}: {
      imports = [
        # The full loader family: services/minecraft/default.nix reads every
        # loader's `enable` flag to pick the dropin dir, and the paper loader
        # reads `services.velocity` for proxy-forwarding defaults, so the
        # sibling modules must be present even though only paper is enabled.
        (paths.modules + "/services/minecraft")
        (paths.modules + "/services/minecraft/fabric")
        (paths.modules + "/services/minecraft/folia")
        (paths.modules + "/services/minecraft/neoforge")
        (paths.modules + "/services/minecraft/paper")
        (paths.modules + "/services/minecraft/purpur")
        (paths.modules + "/services/minecraft/spigot")
        (paths.modules + "/services/minecraft/sponge")
        (paths.modules + "/services/minecraft/vanilla")
        (paths.modules + "/services/velocity")

        # The `ix.*` cross-module surface the minecraft module writes to is
        # declared by the image platform module (lib/image/platform.nix),
        # which cannot be imported here: it bakes OCI-image policy like
        # `boot.isContainer` that conflicts with the test VM. Declare just the
        # option slots the module under test sets; nothing reads them in this
        # test, so permissive types suffice.
        {
          options.ix = {
            extendedAttributes = lib.mkOption {
              type = lib.types.attrsOf lib.types.anything;
              default = {};
            };
            healthChecks = lib.mkOption {
              type = lib.types.attrsOf lib.types.anything;
              default = {};
            };
            networking.portClaims = lib.mkOption {
              type = lib.types.attrsOf lib.types.anything;
              default = {};
            };
          };
        }
      ];

      # Paper wants real memory for startup world gen; the default 1 GiB test
      # VM OOMs the JVM. The heap tracks RAM via MaxRAMPercentage.
      virtualisation = {
        memorySize = 4096;
        cores = 2;
        diskSize = 4096;
      };

      # Mirrors the producer node's server config (examples/minecraft/blocks/
      # producer.nix), minus the fleet-only transport/telemetry legs. A flat
      # world keeps first-boot generation fast; the plugin's behavior does not
      # depend on terrain.
      services.minecraft = {
        enable = true;
        inherit version;
        paper.enable = true;
        openFirewall = true;

        properties = {
          motd = "ix block-events boot smoke test";
          gamemode = "creative";
          level-name = "blocks";
          level-type = "minecraft:flat";
          online-mode = false;
          spawn-protection = 0;
        };

        plugins.block-events = {
          enable = true;
          src = packages.plugin;
          pluginName = "BlockEvents";
        };

        serverFiles."plugins/BlockEvents/config.yml" = {
          logPath = blockLog;
        };
      };

      # Seed the pre-bootstrapped server before the unit's own preStart runs,
      # so paperclip finds valid patched outputs and never goes to the network.
      systemd.services.minecraft.preStart = lib.mkBefore ''
        for tree in versions libraries cache; do
          if [ ! -e "/var/lib/minecraft/$tree" ]; then
            cp -r ${prePatched}/$tree /var/lib/minecraft/
            chmod -R u+w "/var/lib/minecraft/$tree"
          fi
        done
      '';
    };

    testScript = ''
      producer.start()
      producer.wait_for_unit("minecraft.service")

      # onEnable's success line: the plugin opened its JSON Lines log. This is
      # the exact log the original NoSuchMethodError class of failures would
      # have prevented.
      producer.wait_until_succeeds(
          "journalctl -u minecraft.service --grep 'block-events: logging placements to' --quiet",
          timeout=900,
      )

      # Paper finished booting after plugin enable; a plugin that threw during
      # onEnable would be disabled before this line.
      producer.wait_until_succeeds(
          "journalctl -u minecraft.service --grep 'Done .*For help, type' --quiet",
          timeout=900,
      )

      # No enable-time stack trace: Paper logs this exact phrase when a plugin
      # throws out of onEnable, and the plugin's only failure path wraps into
      # UncheckedIOException.
      producer.fail(
          "journalctl -u minecraft.service --grep 'Error occurred while enabling BlockEvents' --quiet"
      )
      producer.fail(
          "journalctl -u minecraft.service --grep 'UncheckedIOException' --quiet"
      )

      # onEnable's side effect, observable outside the journal: the append-only
      # domain-fact log the Kafka shipper would tail exists at the configured
      # path, proving the managed config.yml handoff reached getConfig().
      producer.succeed("test -f ${blockLog}")

      producer.shutdown()
    '';
  }
