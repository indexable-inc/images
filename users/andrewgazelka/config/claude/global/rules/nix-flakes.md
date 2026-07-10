---
paths: "**/flake.nix, "**/flake.lock"
---

# Nix Flakes Guide

## Key Principles

### DO NOT use `flake-utils`

**Wrong:**
```nix
inputs.flake-utils.url = "github:numtide/flake-utils";
flake-utils.lib.eachDefaultSystem (system: ...)
```

**Correct approach** - use a local helper:
```nix
let
  systems = ["x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin"];
  forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
in {
  packages = forAllSystems (system: ...);
}
```

### Use `legacyPackages` not `import`

**Wrong:**
```nix
pkgs = import nixpkgs {inherit system;};
```

**Right:**
```nix
pkgs = nixpkgs.legacyPackages.${system};
```

## Basic Flake Structure

```nix
{
  description = "My project";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs = {
    self,
    nixpkgs,
    ...
  }: let
    systems = ["x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin"];
    forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
  in {
    packages = forAllSystems (system: let
      pkgs = nixpkgs.legacyPackages.${system};
    in {
      default = pkgs.stdenv.mkDerivation {
        pname = "my-package";
        version = "0.1.0";
        src = ./.;
      };
    });

    devShells = forAllSystems (system: let
      pkgs = nixpkgs.legacyPackages.${system};
    in {
      default = pkgs.mkShell {
        packages = [];
      };
    });
  };
}
```

## Common Patterns

### Dev Shell with Checks
```nix
devShells = forAllSystems (system: let
  pkgs = nixpkgs.legacyPackages.${system};
in {
  default = pkgs.mkShell {
    inputsFrom = [self.packages.${system}.default];
    packages = [
      pkgs.rust-analyzer
      pkgs.cargo-watch
    ];
  };
});
```

### Apps (Executable Flake Output)
```nix
apps = forAllSystems (system: {
  default = {
    type = "app";
    program = "${self.packages.${system}.default}/bin/my-program";
  };
});
```

### Overlays (Exporting Packages)

**Always export an overlay** when creating a flake with packages. This lets consumers use `pkgs.yourPackage` instead of verbose `inputs.yourFlake.packages.${system}.default`.

```nix
# In your flake.nix outputs:
overlays.default = final: prev: {
  myPackage = self.packages.${final.system}.default;
};
```

**Consuming overlays** in home-manager/NixOS:
```nix
# In flake.nix - add to pkgs overlays
pkgs = import nixpkgs {
  inherit system;
  overlays = [
    jj-starship.overlays.default
    notify.overlays.default
  ];
};

# Then use anywhere as pkgs.notify, pkgs.jj-starship
home.packages = [ pkgs.notify ];
```

**Overlay vs extraSpecialArgs:**
- Overlay: cleaner, `pkgs.foo` everywhere, requires flake to export overlay
- extraSpecialArgs: fallback when no overlay, verbose `inputs.foo.packages.${system}.default`

## Cross-Compilation to Linux (musl static binaries)

**Use musl targets for cross-compiling Linux binaries from macOS.** musl binaries are statically linked and work on ANY Linux (glibc, musl, Alpine, etc).

Reference: https://nixos.wiki/wiki/Cross_Compiling

```nix
# Cross-compile to Linux from any host using musl
crossBuildFor = hostSystem: targetArch: let
  hostPkgs = pkgsFor hostSystem;

  # Use musl cross toolchains (NOT pkgsCross.gnu64 - glibc won't work!)
  crossPkgs =
    if targetArch == "x86_64"
    then hostPkgs.pkgsCross.musl64
    else hostPkgs.pkgsCross.aarch64-multiplatform-musl;

  cargoTarget =
    if targetArch == "x86_64"
    then "x86_64-unknown-linux-musl"
    else "aarch64-unknown-linux-musl";

  # For Rust with crane:
  craneLib = (crane.mkLib crossPkgs).overrideToolchain rustToolchain;

  commonArgs = {
    src = craneLib.cleanCargoSource ./.;
    strictDeps = true;

    CARGO_BUILD_TARGET = cargoTarget;
    CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";

    # Point cargo to the cross linker
    "CARGO_TARGET_${hostPkgs.lib.toUpper (builtins.replaceStrings ["-"] ["_"] cargoTarget)}_LINKER" =
      "${crossPkgs.stdenv.cc}/bin/${crossPkgs.stdenv.cc.targetPrefix}cc";

    HOST_CC = "${hostPkgs.stdenv.cc}/bin/cc";

    depsBuildBuild = [hostPkgs.stdenv.cc];
    nativeBuildInputs = [crossPkgs.stdenv.cc];
  };
in craneLib.buildPackage commonArgs;
```

**Key points:**
- `pkgsCross.musl64` for x86_64 Linux, `pkgsCross.aarch64-multiplatform-musl` for ARM64
- musl binaries are fully static - no dynamic linking issues
- Works from macOS to Linux without needing a Linux builder
- glibc cross-compilation often fails due to C dependencies (zstd, etc) expecting Linux headers

## Darwin SDK Migration (2024+)

**The `darwin.apple_sdk.frameworks.*` pattern is deprecated.** These are now legacy stubs that error out.

**Wrong:**
```nix
buildInputs = [
  pkgs.darwin.apple_sdk.frameworks.Security
  pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
];
```

**Right:** Don't add framework dependencies explicitly. The new Darwin stdenv includes frameworks automatically when needed. Just use base dependencies:

```nix
buildInputs = [pkgs.openssl];  # Frameworks included by stdenv
```

If you get errors like `error: attribute 'Security' missing`, remove the explicit framework dependencies entirely.

## Validation

**ALWAYS run `nix flake check` before finishing any flake work.**

```bash
nix flake check
```
