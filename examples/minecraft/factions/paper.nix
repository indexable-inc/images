{
  "paper-global.yml" = {
    chunk-loading-basic = {
      player-max-chunk-load-rate = 100;
      player-max-chunk-generate-rate = -1.0;
      player-max-chunk-send-rate = 75;
    };

    chunk-loading-advanced = {
      auto-config-send-distance = true;
      player-max-concurrent-chunk-loads = 0;
      player-max-concurrent-chunk-generates = 0;
    };

    player-auto-save = {
      rate = -1;
      max-per-tick = -1;
    };

    scoreboards = {
      save-empty-scoreboard-teams = false;
      track-plugin-scoreboards = false;
    };

    spam-limiter = {
      incoming-packet-threshold = 300;
      recipe-spam-limit = 20;
      tab-spam-limit = 500;
    };

    spark = {
      enabled = true;
      enable-immediately = false;
    };

    unsupported-settings = {
      allow-headless-pistons = false;
      allow-permanent-block-break-exploits = false;
      # Factions raids need TNT cannons and dupers to behave like
      # players expect; Paper keeps this under unsupported settings.
      allow-piston-duplication = true;
      allow-unsafe-end-portal-teleportation = false;
      perform-username-validation = true;
    };

    watchdog = {
      early-warning-delay = 10000;
      early-warning-every = 5000;
    };
  };

  "paper-world-defaults.yml" = {
    chunks = {
      auto-save-interval = "default";
      delay-chunk-unloads-by = "10s";
      max-auto-save-chunks-per-tick = 24;
      prevent-moving-into-unloaded-chunks = true;
    };

    collisions = {
      allow-player-cramming-damage = false;
      max-entity-collisions = 8;
      only-players-collide = false;
    };

    entities = {
      spawning = {
        count-all-mobs-for-spawning = false;
        despawn-range-shape = "ELLIPSOID";
        per-player-mob-spawns = true;
        spawn-limits = {
          ambient = -1;
          axolotls = -1;
          creature = -1;
          monster = -1;
          underground_water_creature = -1;
          water_ambient = -1;
          water_creature = -1;
        };
      };
    };

    environment = {
      locate-structures-outside-world-border = false;
      nether-ceiling-void-damage-height = 128;
      optimize-explosions = true;
      portal-search-radius = 96;
    };

    misc = {
      disable-sprint-interruption-on-attack = false;
      prevent-tnt-from-moving-in-water = false;
      redstone-implementation = "VANILLA";
      update-pathfinding-on-block-update = true;
    };
  };
}
