{
  settings = {
    allow-end = true;
    warn-on-overload = true;
    query-plugins = false;
    shutdown-message = "Factions is restarting";
    deprecated-verbose = "default";
  };

  spawn-limits = {
    monsters = 70;
    animals = 10;
    water-animals = 5;
    water-ambient = 20;
    water-underground-creature = 5;
    axolotls = 5;
    ambient = 15;
  };

  chunk-gc.period-in-ticks = 600;

  ticks-per = {
    animal-spawns = 400;
    monster-spawns = 1;
    water-spawns = 1;
    water-ambient-spawns = 1;
    water-underground-creature-spawns = 1;
    axolotl-spawns = 1;
    ambient-spawns = 1;
    autosave = 6000;
  };
}
