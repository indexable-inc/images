{
  settings = {
    bungeecord = false;
    debug = false;
    log-named-deaths = false;
    log-villager-deaths = false;
    moved-too-quickly-multiplier = 10.0;
    moved-wrongly-threshold = 0.0625;
    netty-threads = 4;
    player-shuffle = 0;
    restart-on-crash = false;
    sample-count = 12;
    save-user-cache-on-stop-only = false;
    timeout-time = 60;
    user-cache-size = 1000;
  };

  messages = {
    whitelist = "You are not whitelisted on this server.";
    server-full = "Factions is full. Try again soon.";
    restart = "Factions is restarting.";
  };

  commands = {
    log = true;
    tab-complete = 0;
    send-namespaced = true;
  };

  world-settings.default = {
    below-zero-generation-in-existing-chunks = true;
    dragon-death-sound-radius = 0;
    end-portal-sound-radius = 0;
    hanging-tick-frequency = 100;
    wither-spawn-sound-radius = 0;
    item-despawn-rate = 6000;
    mob-spawn-range = 6;
    # Disable Spigot's per-tick TNT throttle for large raid cannon bursts.
    max-tnt-per-tick = -1;
    view-distance = "default";
    simulation-distance = "default";
    verbose = false;

    merge-radius = {
      item = 2.5;
      exp = 4.0;
    };

    ticks-per = {
      hopper-check = 1;
      hopper-transfer = 8;
    };

    hopper-amount = 1;
    hopper-can-load-chunks = false;

    entity-activation-range = {
      animals = 24;
      monsters = 32;
      raiders = 48;
      misc = 0;
      water = 16;
      villagers = 32;
      flying-monsters = 48;
      tick-inactive-villagers = true;
    };

    entity-tracking-range = {
      players = 128;
      animals = 64;
      monsters = 64;
      misc = 128;
      display = 128;
      other = 64;
    };
  };
}
