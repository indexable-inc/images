{errors}: let
  /**
  Major.minor → nixpkgs attribute mapping for the Go toolchain. The
  explicit list means an unknown version fails with the supported set
  rather than `attribute missing` from deep in eval; bump the top entry
  when nixpkgs ships a newer Go.

  `"latest"` follows whatever `pkgs.go` resolves to in the pinned
  nixpkgs, so a caller that does not care about pinning gets the
  channel default without naming a version.
  */
  toolchainsFor = pkgs: {
    "latest" = pkgs.go;
    "1.23" = pkgs.go_1_23;
    "1.25" = pkgs.go_1_25;
    "1.26" = pkgs.go_1_26;
  };
in {
  /**
  Return the Go toolchain for `version`.

  "Toolchain" rather than "compiler" because `go` is a multi-tool:
  `go build`, `go run`, `go test`, `go mod`, plus a bundled standard
  library all live in the same derivation. Picking the version once
  selects all of them as a set, which matches Go's own
  `GOTOOLCHAIN=local` model.

  Arguments:
  - `pkgs`: nixpkgs instance the toolchain comes from.
  - `version`: required, one of `"latest" | "1.23" | "1.25" | "1.26"`.
    Pass `"latest"` to follow `pkgs.go`, or a specific minor when the
    build needs a known compiler version (race-detector internals,
    generic-inference changes, std-lib API additions between releases).

  Example:
  ```nix
  { pkgs, ix, ... }:
  let go = ix.languages.go.toolchain pkgs { version = "1.25"; };
  in { environment.systemPackages = [ go ]; }
  ```
  */
  toolchain = pkgs: args: let
    version = errors.requireArg {
      context = "ix.languages.go.toolchain";
      inherit args;
      name = "version";
    };
  in
    errors.requireAttr {
      context = "ix.languages.go.toolchain: unknown version";
      attrset = toolchainsFor pkgs;
      key = version;
    };

  /**
  Return the Delve debugger (`dlv`).

  Delve loads a binary's DWARF and steps through it; for the line-number
  table to match the program counter the debugger needs to be built
  against the same Go toolchain that compiled the target. devenv handles
  that by rebuilding `delve` with the selected Go via
  `pkg.override { buildGoModule = ... }`. ix leaves the override to the
  caller and returns the floating `pkgs.delve`; a service that pins Go
  and Delve to one minor should rebuild Delve itself with that Go.
  */
  delve = pkgs: _: pkgs.delve;

  /**
  Return the gopls language server package.

  Intended for dev VMs that host an editor. Gopls vendors its own Go
  runtime and is forward-compatible across recent toolchain minors, so
  a single floating package serves any of the `toolchain` versions
  above. Pin and override only if a workspace needs a gopls feature
  older releases lacked.
  */
  languageServer = pkgs: _: pkgs.gopls;
}
