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
  Xcode = 497799835;
  TestFlight = 899247664;
  "Apple Developer" = 640199958;
  "Apple Configurator 2" = 1037126344;
  "Swift Playground" = 1496833156;
  Transporter = 1450874784;

  # ── Apple: pro/creative apps ───────────────────────────────────────────
  "Final Cut Pro" = 424389933;
  "Logic Pro" = 634148309;
  MainStage = 634159523;
  Compressor = 424390742;
  Motion = 434290957;
  GarageBand = 682658836;
  iMovie = 408981434;

  # ── Apple: iWork ───────────────────────────────────────────────────────
  # Universal IDs. The legacy Mac-specific IDs (409201541/409203825/409183694)
  # died: as of 2026-08-10 `mas info` answers "No apps found in the App Store
  # for ADAM ID" for all three, which hard-failed `brew bundle` during darwin
  # activation on hydra (Spotlight indexing is disabled there, so mas cannot
  # detect the installed copies and always re-installs by ID). The universal
  # IDs below were verified the same day with `mas info` on macOS 26.
  Pages = 361309726;
  Numbers = 361304891;
  Keynote = 361285480;

  # ── Productivity & task management ─────────────────────────────────────
  "Things 3" = 904280696;
  "OmniFocus 4" = 1542143627;
  Todoist = 585829637;
  TickTick = 966085870;
  "Microsoft To Do" = 1274495053;
  Structured = 1499198946;
  Fantastical = 975937182;
  "Super Easy Timer" = 1353137878;

  # ── Notes & writing ────────────────────────────────────────────────────
  Bear = 1091189122;
  Ulysses = 1225570693;
  "iA Writer" = 775737590;
  Drafts = 1435957248;
  Agenda = 1287445660;
  "Day One" = 1055511498;
  Goodnotes = 1444383602;
  Notability = 360593530;
  Craft = 1487937127;
  CotEditor = 1024640650;

  # ── Microsoft Office & services ────────────────────────────────────────
  "Microsoft Word" = 462054704;
  "Microsoft Excel" = 462058435;
  "Microsoft PowerPoint" = 462062816;
  "Microsoft Outlook" = 985367838;
  "Microsoft OneNote" = 784801555;
  OneDrive = 823766827;
  "Windows App" = 1295203466;

  # ── Communication ──────────────────────────────────────────────────────
  "Slack for Desktop" = 803453959;
  Telegram = 747648890;
  "WhatsApp Messenger" = 310633997;
  LINE = 539883307;

  # ── Safari extensions ──────────────────────────────────────────────────
  "AdGuard for Safari" = 1440147259;
  "Wipr 2" = 1662217862;
  "1Blocker" = 1365531024;
  Vinegar = 1591303229;
  "Baking Soda" = 1601151613;
  Userscripts = 1463298887;
  Tampermonkey = 6738342400;
  "1Password for Safari" = 1569813296;
  "Dark Reader for Safari" = 1438243180;

  # ── Security & networking ──────────────────────────────────────────────
  WireGuard = 1451685025;
  Tailscale = 1475387142;
  Bitwarden = 1352778147;
  "Speedtest by Ookla" = 1153157709;

  # ── Utilities ──────────────────────────────────────────────────────────
  Amphetamine = 937984704;
  Magnet = 441258766;
  "Hidden Bar" = 1452453066;
  "The Unarchiver" = 425424353;
  Keka = 470158793;
  DaisyDisk = 411643860;
  CleanMyMac = 1339170533;
  Velja = 1607635845;
  Yoink = 457622435;
  Paste = 967805235;
  Pastebot = 1179623856;
  "Screens 5" = 1663047912;
  Actions = 1586435171;
  Shortery = 1594183810;
  "Gestimer 2" = 6447125648;
  Dato = 1470584107;
  RocketSim = 1504940162;

  # ── Media & creative (third-party) ─────────────────────────────────────
  "Pixelmator Pro" = 1289583905;
  Photomator = 1444636541;
  Infuse = 1136220934;
  djay = 450527929;
  Shazam = 897118787;
  Endel = 1346247457;

  # ── Reading, weather & lifestyle ───────────────────────────────────────
  "Reeder." = 6475002485;
  "Reeder Classic." = 1529448980;
  "Amazon Kindle" = 302584613;
  Parcel = 375589283;
  "CARROT Weather" = 993487541;
  Mela = 1568924476;
  "Flighty – Live Flight Tracker" = 1358823008;
  Portal = 1436994560;
  "Home Assistant" = 1099568401;
}
