{
  description = "ix example: full-stack Biff 2 Todo App";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    clj-nix = {
      url = "github:jlesquembre/clj-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    index,
    nixpkgs,
    clj-nix,
    ...
  }: let
    system = "x86_64-linux";
    pkgs = nixpkgs.legacyPackages."${system}";
    projectCoordinate = "com.example/biff-todo-app";
    tailwindBinary = pkgs.fetchurl {
      url = "https://github.com/tailwindlabs/tailwindcss/releases/download/v4.3.0/tailwindcss-linux-x64";
      hash = "sha256-c/DlRZBU5c+qirbzuUDz++DxPMf9g7wk58ZVAzwgNAA=";
    };
    # Tailwind's standalone executable is a Bun single-file binary. Patching
    # the ELF in place corrupts Bun's appended payload, so invoke the original
    # fixed-output artifact through Nix's dynamic loader instead.
    tailwind = index.lib.writeRustApplication pkgs {
      name = "tailwindcss";
      text = ''
        fn main() {
            use std::os::unix::process::CommandExt;

            let err = std::process::Command::new("${pkgs.stdenv.cc.bintools.dynamicLinker}")
                .arg("${tailwindBinary}")
                .args(std::env::args_os().skip(1))
                .env("LD_LIBRARY_PATH", "${nixpkgs.lib.makeLibraryPath [pkgs.stdenv.cc.cc.lib]}")
                .exec();
            eprintln!("tailwindcss: exec failed: {err}");
            std::process::exit(127);
        }
      '';
    };
    datastar = pkgs.fetchurl {
      url = "https://cdn.jsdelivr.net/gh/starfederation/datastar@v1.0.1/bundles/datastar.js";
      hash = "sha256-VHaM80mFvgIpxyKfHflGn70y4qDAm0o/HoGtjE1oQNo=";
    };
    biffApp = clj-nix.lib.mkCljApp {
      inherit pkgs;
      modules = [
        {
          projectSrc = ./.;
          name = projectCoordinate;
          version = "0.1.0";
          main-ns = "com.example.todo-app";
          java-opts = [
            "-Dclojure.main.report=stderr"
            "-XX:-OmitStackTraceInFastThrow"
            "-XX:+CrashOnOutOfMemoryError"
          ];
          builder-extra-inputs = [tailwind];
          builder-preBuild = ''
            mkdir -p target/resources/public/css target/resources/public/js
            tailwindcss \
              --input resources/tailwind.css \
              --output target/resources/public/css/main.css \
              --minify
            install -m 0444 ${datastar} target/resources/public/js/datastar.js
          '';
        }
      ];
    };
    importIx = index.lib.importIxWasm;
    vm = importIx ./default.ix {inherit index biffApp;};
    vmTest = import ./vm-test.nix {inherit biffApp index pkgs;};
  in {
    packages.${system} = {
      default = biffApp;
      deps-lock = clj-nix.packages.${system}.deps-lock;
    };
    checks.${system}.biff-todo-app-vm = vmTest;
    ix.default = vm;
    inherit (vm) nixosConfigurations;
  };
}
