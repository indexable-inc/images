{world}: {
  autoReloadIgnored = [
    "Vault"
    "LuckPerms"
    "PlaceholderAPI"
    "TeamsAPI"
    "Essentials"
    "EssentialsSpawn"
    "WorldEdit"
    "WorldGuard"
    "CoreProtect"
    "EternalEconomy"
    "QuickShop-Hikari"
    "TradePost"
    "PvPIndexFactions"
    "CombatLog"
    "BlueMap"
    "Skript"
  ];

  enabled = {
    luckperms = {};
    teams-api = {};
    placeholderapi = {};
    vaultunlocked = {};
    essentialsx = {};
    essentialsx-spawn = {};
    coreprotect = {};
    eternaleconomy = {};
    quickshop-hikari = {};
    tradepost = {};
    worldedit = {};
    worldguard = {};
    terraformgenerator.worlds = [
      world.name
      "${world.name}_nether"
      "${world.name}_the_end"
    ];
    pvpindex-factions = {};
    combatlogplugin = {};
    simple-voice-chat = {};
    distant-horizons-support = {};
    bluemap = {};
    skript = {};
  };
}
