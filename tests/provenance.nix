# Eval test for the provenance walker + home module (whence, #2413).
# Evaluates a real home-manager configuration through
# modules/home/provenance.nix and asserts the manifest links deployed paths
# back to their defining nix sites: a literal `home.file` entry carries the
# fixture's file:line, an `xdg.configFile` entry records both the user site
# and the home-manager wiring hop, and a `programs.htop.settings`-driven
# file records the settings chain. Eval-only: nothing from the evaluated
# configuration is built.
{
  lib,
  pkgs,
  ix,
  paths,
  home-manager,
}: let
  testRev = "0000000000000000000000000000000000000000";

  hmConfig =
    (home-manager.lib.homeManagerConfiguration {
      inherit pkgs;
      modules = [
        (import (paths.root + "/modules/home/provenance.nix") {inherit (ix) provenance;})
        ./fixtures/provenance-home.nix
        {
          provenance = {
            enable = true;
            rev = testRev;
          };
        }
      ];
    }).config;

  inherit (hmConfig.provenance.entries) files;

  fixtureSuffix = "fixtures/provenance-home.nix";
  siteInFixture = site: lib.hasSuffix fixtureSuffix site.file && site.line != null;

  fileEntry = files."provenance-test.txt" or null;
  xdgEntry = files.".config/provenance-test/config.toml" or null;
  # Current home-manager deploys htop's config as `.config/htop` (a
  # directory source); older revisions wrote `.config/htop/htoprc`. Scan by
  # prefix so the assertion tracks the settings chain, not HM's layout.
  htopEntry = let
    keys = lib.filter (key: lib.hasPrefix ".config/htop" key) (builtins.attrNames files);
  in
    if keys == []
    then null
    else files.${builtins.head keys};

  assertions = [
    {
      assertion = fileEntry != null && siteInFixture fileEntry;
      message = "home.file entry should resolve to its literal fixture site (file:line)";
    }
    {
      assertion = fileEntry != null && fileEntry.rev == testRev;
      message = "manifest entries should carry the configured flake rev";
    }
    {
      assertion =
        fileEntry != null && fileEntry.source != null && lib.hasPrefix builtins.storeDir fileEntry.source;
      message = "home.file entry should record its store source";
    }
    {
      assertion = fileEntry != null && fileEntry.drv != null && lib.hasSuffix ".drv" fileEntry.drv;
      message = "a text-generated home.file entry should record its deriver";
    }
    {
      assertion = xdgEntry != null && siteInFixture xdgEntry;
      message = "the xdg entry's primary site should be the user's literal definition";
    }
    {
      assertion = xdgEntry != null && lib.any (site: !(lib.hasSuffix fixtureSuffix site.file)) xdgEntry.definitions;
      message = "the xdg entry should also record the home-manager wiring hop";
    }
    {
      assertion =
        htopEntry
        != null
        && lib.any (
          chain:
            chain.option
            == "programs.htop.settings"
            && lib.any siteInFixture chain.definitions
        )
        htopEntry.settings;
      message = "settings-driven files should chain back to the user's programs.*.settings site";
    }
    {
      assertion = htopEntry != null && !(siteInFixture htopEntry);
      message = "the settings-driven file's own definition should be the wiring module, not the fixture";
    }
    {
      assertion = lib.hasInfix "provenance.json" hmConfig.home.extraBuilderCommands;
      message = "enabling provenance should link provenance.json into the generation builder";
    }
  ];

  failures = map (a: a.message) (lib.filter (a: !a.assertion) assertions);
in
  assert lib.assertMsg (failures == []) (
    "provenance:\n  " + lib.concatStringsSep "\n  " failures
  );
    pkgs.runCommand "ix-test-provenance" {__structuredAttrs = true;} ''
      mkdir -p "$out"
    ''
