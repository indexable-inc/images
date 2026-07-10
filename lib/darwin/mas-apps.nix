# General catalog of Mac App Store app IDs for nix-darwin `homebrew.masApps`.
#
# nix-darwin's `homebrew.masApps` wants `name -> numeric MAS ID`; the ID is
# what `mas install` actually uses and the name is a human label. Getting an
# ID wrong is not cosmetic: with `onActivation.cleanup = "zap"` an undeclared
# or mis-declared app is *uninstalled* on the next switch. This file is the
# shared, verified source of truth so per-user modules can select from it
# instead of each hand-maintaining (and re-verifying) their own numbers:
#
#   { lib, ... }: let
#     masCatalog = import ./relative/path/to/lib/darwin/mas-apps.nix;
#   in {
#     homebrew.masApps = lib.getAttrs ["Xcode" "Things 3"] masCatalog;
#   }
#
# `lib.getAttrs` throws on a missing name, so a typo fails at eval instead of
# silently zapping an app.
#
# Every ID here was verified against the iTunes lookup/search API
# (https://itunes.apple.com/lookup?id=<id>) or comes from a live `mas list`
# on a machine that has the app installed. When adding an entry, verify the
# ID the same way — do not copy numbers from blog posts or from memory.
# Some long-lived Mac apps have legacy Mac-specific IDs (e.g. the iWork trio)
# that `mas` resolves fine but the lookup API no longer returns; those are
# annotated inline and should not be "fixed" to the newer universal IDs
# without testing `mas install` with them first.
{
  # ── Apple: developer tools ─────────────────────────────────────────────
  Xcode = 497_799_835;
  TestFlight = 899_247_664;
  "Apple Developer" = 640_199_958;
  "Apple Configurator 2" = 1_037_126_344;
  "Swift Playground" = 1_496_833_156;
  Transporter = 1_450_874_784;

  # ── Apple: pro/creative apps ───────────────────────────────────────────
  "Final Cut Pro" = 424_389_933;
  "Logic Pro" = 634_148_309;
  MainStage = 634_159_523;
  Compressor = 424_390_742;
  Motion = 434_290_957;
  GarageBand = 682_658_836;
  iMovie = 408_981_434;

  # ── Apple: iWork ───────────────────────────────────────────────────────
  # Legacy Mac-specific IDs from `mas list`; the lookup API resolves only the
  # newer universal IDs (361309726/361304891/361285480), but these still
  # install correctly via `mas`. Keep as-is.
  Pages = 409_201_541;
  Numbers = 409_203_825;
  Keynote = 409_183_694;

  # ── Productivity & task management ─────────────────────────────────────
  "Things 3" = 904_280_696;
  "OmniFocus 4" = 1_542_143_627;
  Todoist = 585_829_637;
  TickTick = 966_085_870;
  "Microsoft To Do" = 1_274_495_053;
  Structured = 1_499_198_946;
  Fantastical = 975_937_182;
  "Super Easy Timer" = 1_353_137_878;

  # ── Notes & writing ────────────────────────────────────────────────────
  Bear = 1_091_189_122;
  Ulysses = 1_225_570_693;
  "iA Writer" = 775_737_590;
  Drafts = 1_435_957_248;
  Agenda = 1_287_445_660;
  "Day One" = 1_055_511_498;
  Goodnotes = 1_444_383_602;
  Notability = 360_593_530;
  Craft = 1_487_937_127;
  CotEditor = 1_024_640_650;

  # ── Microsoft Office & services ────────────────────────────────────────
  "Microsoft Word" = 462_054_704;
  "Microsoft Excel" = 462_058_435;
  "Microsoft PowerPoint" = 462_062_816;
  "Microsoft Outlook" = 985_367_838;
  "Microsoft OneNote" = 784_801_555;
  OneDrive = 823_766_827;
  "Windows App" = 1_295_203_466;

  # ── Communication ──────────────────────────────────────────────────────
  "Slack for Desktop" = 803_453_959;
  Telegram = 747_648_890;
  "WhatsApp Messenger" = 310_633_997;
  LINE = 539_883_307;

  # ── Safari extensions ──────────────────────────────────────────────────
  "AdGuard for Safari" = 1_440_147_259;
  "Wipr 2" = 1_662_217_862;
  "1Blocker" = 1_365_531_024;
  Vinegar = 1_591_303_229;
  "Baking Soda" = 1_601_151_613;
  Userscripts = 1_463_298_887;
  Tampermonkey = 6_738_342_400;
  "1Password for Safari" = 1_569_813_296;
  "Dark Reader for Safari" = 1_438_243_180;

  # ── Security & networking ──────────────────────────────────────────────
  WireGuard = 1_451_685_025;
  Tailscale = 1_475_387_142;
  Bitwarden = 1_352_778_147;
  "Speedtest by Ookla" = 1_153_157_709;

  # ── Utilities ──────────────────────────────────────────────────────────
  Amphetamine = 937_984_704;
  Magnet = 441_258_766;
  "Hidden Bar" = 1_452_453_066;
  "The Unarchiver" = 425_424_353;
  Keka = 470_158_793;
  DaisyDisk = 411_643_860;
  CleanMyMac = 1_339_170_533;
  Velja = 1_607_635_845;
  Yoink = 457_622_435;
  Paste = 967_805_235;
  Pastebot = 1_179_623_856;
  "Screens 5" = 1_663_047_912;
  Actions = 1_586_435_171;
  Shortery = 1_594_183_810;
  "Gestimer 2" = 6_447_125_648;
  Dato = 1_470_584_107;
  RocketSim = 1_504_940_162;

  # ── Media & creative (third-party) ─────────────────────────────────────
  "Pixelmator Pro" = 1_289_583_905;
  Photomator = 1_444_636_541;
  Infuse = 1_136_220_934;
  djay = 450_527_929;
  Shazam = 897_118_787;
  Endel = 1_346_247_457;

  # ── Reading, weather & lifestyle ───────────────────────────────────────
  "Reeder." = 6_475_002_485;
  "Reeder Classic." = 1_529_448_980;
  "Amazon Kindle" = 302_584_613;
  Parcel = 375_589_283;
  "CARROT Weather" = 993_487_541;
  Mela = 1_568_924_476;
  "Flighty – Live Flight Tracker" = 1_358_823_008;
  Portal = 1_436_994_560;
  "Home Assistant" = 1_099_568_401;
}
