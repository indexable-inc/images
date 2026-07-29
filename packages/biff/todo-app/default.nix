# Biff 2 todo app: the reading list's write path split into authentication,
# user-scoped writes, background work, and Datastar live-query updates.
#
# Built as one content-addressed derivation per Clojure namespace
# ([`lib/build/clj-unit.nix`](../../../lib/build/clj-unit.nix)) over a
# dependency closure of one fetch derivation per artifact
# ([`lib/build/clj-lock.nix`](../../../lib/build/clj-lock.nix)). The
# deployment lives in `modules/services/biff-todo-app`.
{
  ix,
  lib,
  pkgs,
  ...
}: let
  # Browser asset versions and hashes live beside the package rather than
  # inline: repo policy is no hash literals in tracked .nix, and a pin
  # carrying `url` joins `nix run .#update` so its hash refreshes mechanically.
  pins = ix.pins.loadPins ./pins.json;

  # Tailwind ships one standalone binary per platform, so the pin is named
  # per platform rather than per tool.
  tailwindPinNames = {
    aarch64-darwin = "tailwindcss-macos-arm64";
    x86_64-linux = "tailwindcss-linux-x64";
  };
  system = pkgs.stdenv.hostPlatform.system;
  tailwindPin =
    pins.${
      tailwindPinNames.${system}
      or (throw "biff-todo-app: no tailwindcss binary pinned for ${system}; add one to packages/biff/todo-app/pins.json")
    };

  tailwindBinary = pkgs.fetchurl {inherit (tailwindPin) url hash;};

  # Tailwind's standalone executable is a Bun single-file binary: Bun appends
  # its payload after the ELF image, so patchelf corrupts it. Invoking the
  # untouched artifact through the dynamic loader is the only way to run it.
  tailwind = ix.writeRustApplication pkgs {
    name = "tailwindcss";
    text = ''
      fn main() {
          use std::os::unix::process::CommandExt;

          let err = std::process::Command::new("${pkgs.stdenv.cc.bintools.dynamicLinker}")
              .arg("${tailwindBinary}")
              .args(std::env::args_os().skip(1))
              .env("LD_LIBRARY_PATH", "${lib.makeLibraryPath [pkgs.stdenv.cc.cc.lib]}")
              .exec();
          eprintln!("tailwindcss: exec failed: {err}");
          std::process::exit(127);
      }
    '';
  };

  datastar = pkgs.fetchurl {inherit (pins.datastar) url hash;};

  # Generated browser assets, kept out of the compile units: they are runtime
  # classpath entries, and rebuilding CSS must not recompile a namespace.
  # Vendoring Datastar here is what lets the deployed page work with no CDN.
  browserAssets =
    pkgs.runCommand "biff-todo-app-assets" {
      nativeBuildInputs = [tailwind];
      strictDeps = true;
    } ''
      mkdir -p "$out/public/css" "$out/public/js"
      tailwindcss \
        --input ${./resources/tailwind.css} \
        --output "$out/public/css/main.css" \
        --minify
      install -m 0444 ${datastar} "$out/public/js/datastar.js"
    '';

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./src
      ./resources
      ./deps.edn
    ];
  };
in
  ix.cljUnit.buildApplication {
    pname = "biff-todo-app";
    version = "0.1.0";
    inherit src;
    mainNamespace = "com.example.todo-app";
    sourceRoots = ["src"];
    resourceRoots = ["resources"];
    extraClasspath = [browserAssets];
    classpathJars = ix.cljLock.classpathFor {lock = ./deps-lock.json;};
    meta = {
      description = "Biff 2 todo app: auth, user-scoped writes, background jobs, Datastar live queries";
      mainProgram = "biff-todo-app";
    };
  }
