{errors}: let
  validVendors = [
    "gcc"
    "clang"
  ];

  /**
  Vendor → version table for C/C++ compilers. gcc and clang have
  independent release cadences and version ranges; an unknown
  `(vendor, version)` pair fails with the supported set for that
  vendor rather than `attribute missing` from deep in eval.

  The C++ standard library is what actually ships with the compiler:
  gcc bundles libstdc++, clang on NixOS uses gcc's libstdc++ by
  default, and switching vendors changes both the front end and the
  ABI of `<vector>`. Pick the vendor explicitly so cross-vendor link
  failures surface at config time, not at link time.
  */
  compilersFor = pkgs: {
    gcc = {
      "latest" = pkgs.gcc;
      "9" = pkgs.gcc9;
      "10" = pkgs.gcc10;
      "11" = pkgs.gcc11;
      "12" = pkgs.gcc12;
      "13" = pkgs.gcc13;
      "14" = pkgs.gcc14;
      "15" = pkgs.gcc15;
    };
    clang = {
      "latest" = pkgs.clang;
      "16" = pkgs.clang_16;
      "17" = pkgs.clang_17;
      "18" = pkgs.clang_18;
      "19" = pkgs.clang_19;
      "20" = pkgs.clang_20;
      "21" = pkgs.clang_21;
      "22" = pkgs.clang_22;
    };
  };
in {
  /**
  Return a C/C++ compiler package for the requested vendor and version.

  "compiler" rather than "toolchain" because the gcc and clang
  derivations are just the front-end + driver; build orchestration
  (cmake, ninja, meson, make) is exposed as separate siblings so an
  image can pick a non-default build tool without re-resolving the
  compiler choice.

  Arguments:
  - `pkgs`: nixpkgs instance the compiler comes from.
  - `vendor`: required, `"gcc" | "clang"`. Pick `"gcc"` to match the
    platform default (saves a second C++ stdlib in the closure); pick
    `"clang"` when an upstream needs libc++ or AddressSanitizer features
    clang ships ahead of.
  - `version`: required, `"latest"` or a vendor-specific major. gcc
    covers 9-15, clang covers 16-22.

  Example:
  ```nix
  { pkgs, ix, ... }:
  let
    gcc = ix.languages.cpp.compiler pkgs { vendor = "gcc"; version = "14"; };
    cmake = ix.languages.cpp.cmake pkgs { };
  in {
    environment.systemPackages = [ gcc cmake ];
  }
  ```
  */
  compiler = pkgs: args: let
    vendor = errors.requireArg {
      context = "ix.languages.cpp.compiler";
      inherit args;
      name = "vendor";
    };
    version = errors.requireArg {
      context = "ix.languages.cpp.compiler";
      inherit args;
      name = "version";
    };

    checkedVendor = errors.assertEnum {
      name = "ix.languages.cpp.compiler.vendor";
      value = vendor;
      valid = validVendors;
    };

    vendorTable = errors.requireAttr {
      context = "ix.languages.cpp.compiler: vendor table";
      attrset = compilersFor pkgs;
      key = checkedVendor;
    };
  in
    errors.requireAttr {
      context = "ix.languages.cpp.compiler: unknown version for vendor '${checkedVendor}'";
      attrset = vendorTable;
      key = version;
    };

  /**
  Return CMake. The default modern build orchestrator for C/C++
  projects; reads `CMakeLists.txt`, drives ninja/make underneath.
  */
  cmake = pkgs: _: pkgs.cmake;

  /**
  Return Ninja. Generator backend used by CMake and Meson when low
  incremental-build latency matters; faster than `make` for parallel
  rebuilds because it does not re-stat the entire dep graph.
  */
  ninja = pkgs: _: pkgs.ninja;

  /**
  Return Meson. Build-system alternative to CMake, used by GNOME,
  systemd, and a non-trivial slice of nixpkgs C-language packages.
  */
  meson = pkgs: _: pkgs.meson;

  /**
  Return GNU Make. Lowest common denominator; still right when a
  project's `Makefile` is the build interface and Ninja/CMake would
  be added complexity.
  */
  make = pkgs: _: pkgs.gnumake;

  /**
  Return clang-tools (provides `clangd`, the C/C++ language server,
  plus `clang-format`, `clang-tidy`). Intended for dev VMs that host
  an editor.

  `clangd` works against either gcc or clang projects because it
  reads `compile_commands.json` for flags; the bundled clang front
  end is what parses the source, independent of which compiler
  actually builds the binaries.
  */
  languageServer = pkgs: _: pkgs.clang-tools;
}
