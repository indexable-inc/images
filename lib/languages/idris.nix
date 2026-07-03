_: {
  /**
  Return the Idris2 compiler.

  Idris2 is a purely functional, dependently-typed language with a
  single canonical compiler in nixpkgs (`pkgs.idris2`). The compiler
  is self-hosted but bootstraps through Chez Scheme: the default
  `chez` codegen emits Scheme that `pkgs.chez` runs, and the
  `pkgs.idris2` derivation already wires Chez in as a runtime
  dependency, so an image consuming this helper does not separately
  pull Chez into the closure.

  Codegen is selected per invocation with `idris2 --codegen <name>`
  rather than at build time. The bundled backends are `chez`
  (default), `racket`, `gambit`, `node`, `javascript`, `refc` (the
  C-backed reference codegen), and `vmcode-interp`. Only the `chez`
  path is tested on a stock `pkgs.idris2`; reaching for another
  backend means dropping the matching runtime into the image
  (`pkgs.racket`, `pkgs.gambit`, `pkgs.nodejs`, or a C toolchain for
  `refc`).

  `pkgs.idris2Packages` exposes the Idris2 library set if a later
  helper needs to add a `package` selector; this entry keeps the
  minimal compiler-only surface that the other one-binary helpers in
  this namespace use.

  Arguments:
  - `pkgs`: nixpkgs instance the compiler comes from.

  Example:
  ```nix
  { pkgs, ix, ... }:
  let idris = ix.languages.idris.compiler pkgs { };
  in { environment.systemPackages = [ idris ]; }
  ```
  */
  compiler = pkgs: _: pkgs.idris2;
}
