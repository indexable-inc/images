{errors}: let
  /**
  Zig is pre-1.0 and breaks source compatibility between 0.x minors;
  the explicit table makes "what version do I need" the actual
  question the helper answers, rather than letting `pkgs.zig` float
  a build out from under a project that pinned to 0.14 syntax.
  */
  toolchainsFor = pkgs: {
    "latest" = pkgs.zig;
    "0.12" = pkgs.zig_0_12;
    "0.13" = pkgs.zig_0_13;
    "0.14" = pkgs.zig_0_14;
    "0.15" = pkgs.zig_0_15;
    "0.16" = pkgs.zig_0_16;
  };
in {
  /**
  Return the Zig toolchain for `version`.

  `zig` is one binary that builds, tests, runs, and cross-compiles;
  it also vendors clang for C interop, so a project that mixes Zig
  and C does not need a separate compiler. The build system is the
  `build.zig` script in the project root — there is no separate
  cmake/ninja sibling here.

  Arguments:
  - `pkgs`: nixpkgs instance the toolchain comes from.
  - `version`: required, one of `"latest" | "0.12" | "0.13" | "0.14"
    | "0.15" | "0.16"`. Pin a specific minor because Zig is pre-1.0
    and breaks source-level compatibility regularly.

  Example:
  ```nix
  { pkgs, ix, ... }:
  let zig = ix.languages.zig.toolchain pkgs { version = "0.14"; };
  in { environment.systemPackages = [ zig ]; }
  ```
  */
  toolchain = pkgs: args: let
    version = errors.requireArg {
      context = "ix.languages.zig.toolchain";
      inherit args;
      name = "version";
    };
  in
    errors.requireAttr {
      context = "ix.languages.zig.toolchain: unknown version";
      attrset = toolchainsFor pkgs;
      key = version;
    };

  /**
  Return the Zig language server (`zls`).

  zls is version-coupled to a specific Zig release: an editor pointed
  at a zls built against 0.14 talking to a 0.13 compiler silently
  drops completions. The floating `pkgs.zls` matches `pkgs.zig`; pin
  both together if `toolchain` is on a non-default minor.
  */
  languageServer = pkgs: _: pkgs.zls;
}
