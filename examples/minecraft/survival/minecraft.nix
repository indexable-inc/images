{ix, ...}: let
  forwardingSecret = "ix-survival-example-forwarding-secret-change-me";
  tags = ix.minecraft.nbt;
in {
  services = {
    velocity = {
      enable = true;
      motd = "<green>ix Survival</green> <gray>| Java and Bedrock</gray>";
      # Velocity renders MiniMessage tags into plain text before answering
      # SLP. Substring matching here proves the proxy is actually the one
      # responding on 25565, not a stray backend or stale image.
      health.motdContains = ["ix Survival"];
      forwarding.secret = forwardingSecret;
      servers.survival = "127.0.0.1:25566";
      try = ["survival"];
    };

    geyser = {
      enable = true;
      bedrock = {
        motd1 = "ix Survival";
        motd2 = "Java and Bedrock";
        serverName = "ix Survival";
      };
    };

    floodgate.enable = true;

    minecraft = {
      enable = true;
      version = "26.1.2";
      paper.enable = true;
      port = 25566;
      openFirewall = false;

      properties = {
        motd = "ix Survival";
        difficulty = "hard";
        gamemode = "survival";
        level-name = "survival";
        max-players = 120;
        online-mode = false;
        spawn-protection = 0;
        view-distance = 16;
        simulation-distance = 10;
        pvp = true;
      };

      configFiles."paper-global.yml".proxies.velocity = {
        enabled = true;
        secret = forwardingSecret;
        online-mode = true;
      };

      serverFiles."spigot.yml".settings = {
        bungeecord = false;
        restart-on-crash = false;
      };

      # Demo: ship a tiny vanilla datapack whose only payload is a structure
      # template. An operator (or a function in another datapack) can paste
      # the structure with `/place template ix:zombie_arena ~ ~ ~`. The point
      # is to exercise the typed-NBT pipeline end to end: Nix-side helpers
      # tag each value with its NBT kind, `mkMinecraftNbtFormat` generates
      # the binary NBT file, and the server loads it on world start without
      # ever writing back.
      datapacks.ix-spawner-arena.files."data/ix/structure/zombie_arena.nbt" = tags.compound {
        # DataFixer upgrades older structure files on load. Using the
        # 1.21.11 stamp here is intentional: it is the most recent
        # release whose server jar ships with Mojang deobfuscation
        # mappings, so the format is verifiable against decompiled
        # source via `nix run .#mc-source -- 1.21.11`.
        DataVersion = tags.int 4325;

        # `size` is a List of three Int tags, not an IntArray. Same
        # for `pos` below. StructureTemplate.save() in vanilla writes
        # `newIntegerList(...)`, which is ListTag<IntTag>.
        size = tags.list [
          (tags.int 1)
          (tags.int 1)
          (tags.int 1)
        ];

        palette = [
          {Name = tags.string "minecraft:spawner";}
        ];

        blocks = [
          {
            pos = tags.list [
              (tags.int 0)
              (tags.int 0)
              (tags.int 0)
            ];
            state = tags.int 0;
            nbt = tags.compound {
              # Every spawner timing field is a Short (signed 16-bit).
              # Passing an Int here loads silently and then misbehaves
              # because the server reads the wrong number of bytes:
              # the typed wrappers make the width explicit so the
              # bug cannot happen.
              Delay = tags.short 20;
              MinSpawnDelay = tags.short 200;
              MaxSpawnDelay = tags.short 800;
              SpawnCount = tags.short 4;
              SpawnRange = tags.short 4;
              MaxNearbyEntities = tags.short 6;
              RequiredPlayerRange = tags.short 16;
              # `entity` wrapper around `id` is the post-1.18 shape.
              # Pre-1.18 wrote `id` as a sibling of `SpawnData`.
              SpawnData = tags.compound {
                entity = tags.compound {
                  id = tags.string "minecraft:zombie";
                };
              };
            };
          }
        ];

        entities = [];
      };
    };
  };
}
