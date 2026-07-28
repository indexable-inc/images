# andrewgazelka's personal profile composition (#3899): every wiring that
# names users/andrewgazelka, kept out of flake.nix so the flake only consumes
# the composed surfaces. The shared home-module instances arrive as args from
# lib/home-modules.nix, which composes both module surfaces.
{
  lib,
  ix,
  paths,
  indexPackages,
  home-manager,
  nixpkgs,
  # Wraps `import <path> args` with the path as `_file` so consumers'
  # `definitionsWithLocations` attribute definitions to the module's own
  # source file (#3938). Shared from lib/home-modules.nix.
  importApply,
  claudeCodeModule,
  codexModule,
  mutableFilesModule,
  provenanceModule,
  macosGuestsModule,
}: let
  personalRoot = paths.users + "/andrewgazelka";
  configRoot = personalRoot + "/config";
  optionsModule = personalRoot + "/options.nix";
  personalServicesModule = importApply (personalRoot + "/home.nix") {
    inherit indexPackages ix claudeCodeModule;
    portableServicesModule = ix.portableServices.homeModule;
  };
  portableModule = personalRoot + "/profiles/portable.nix";
  developmentModule = importApply (personalRoot + "/profiles/development.nix") {
    agentLua = paths.modules + "/profiles/base/nvim/agent.lua";
    inherit configRoot;
  };
in {
  inherit personalServicesModule portableModule developmentModule;
  workstationModule = importApply (personalRoot + "/profiles/workstation.nix") {
    inherit
      indexPackages
      personalServicesModule
      ix
      codexModule
      configRoot
      mutableFilesModule
      provenanceModule
      optionsModule
      ;
    tmuxModule = paths.modules + "/home/tmux.nix";
    activationTimingModule = paths.modules + "/home/activation-timing.nix";
  };
  darwinHomeModule = importApply (personalRoot + "/profiles/darwin-home.nix") {
    inherit
      indexPackages
      ix
      configRoot
      optionsModule
      ;
    ghosttyModule = configRoot + "/home/ghostty.nix";
    raycastModule = paths.modules + "/home/raycast.nix";
    inherit macosGuestsModule;
    guestsModule = importApply (personalRoot + "/guests/default.nix") {inherit indexPackages;};
  };
  # The dependency-light personal profile pair (portable + development),
  # composed as a real homeManagerConfiguration so `checks` can force its
  # activation package on every dev system. The throwing extraSpecialArgs pin
  # the "light" property: these profiles must never reach the consuming
  # flake, its inputs, or the index package set.
  lightProfileFor = system:
    home-manager.lib.homeManagerConfiguration {
      pkgs = import nixpkgs {
        inherit system;
        config = {};
      };
      extraSpecialArgs = {
        inputs = throw "light personal profiles must not access consumer inputs";
        self = throw "light personal profiles must not access the consuming flake";
        indexPackages = throw "light personal profiles must not access index packages";
      };
      modules = [
        portableModule
        developmentModule
        {
          home = {
            username = "profile-test";
            homeDirectory =
              if lib.hasSuffix "darwin" system
              then "/Users/profile-test"
              else "/home/profile-test";
          };
        }
      ];
    };
}
