{errors}: let
  /**
  OCaml major.minor → versioned package set under `pkgs.ocaml-ng`.
  Each entry is a *whole package set* (compiler plus libraries built
  against it); `dune`, `ocaml-lsp`, `utop`, and friends come from the
  same set so they share a runtime. Picking 5.x vs 4.14 is the
  load-bearing decision because 5.0 changed the runtime (effects,
  multicore) and split the library ecosystem in two.
  */
  ocamlPackagesFor = pkgs: {
    "latest" = pkgs.ocaml-ng.ocamlPackages_latest;
    "4.14" = pkgs.ocaml-ng.ocamlPackages_4_14;
    "5.1" = pkgs.ocaml-ng.ocamlPackages_5_1;
    "5.2" = pkgs.ocaml-ng.ocamlPackages_5_2;
    "5.3" = pkgs.ocaml-ng.ocamlPackages_5_3;
    "5.4" = pkgs.ocaml-ng.ocamlPackages_5_4;
  };

  packageSetFor = pkgs: version:
    errors.requireAttr {
      context = "ix.languages.ocaml: unknown version";
      attrset = ocamlPackagesFor pkgs;
      key = version;
    };

  requireVersion = context: args:
    errors.requireArg {
      inherit context args;
      name = "version";
    };
in {
  /**
  Return the OCaml compiler for `version`.

  "compiler" matches the ecosystem's term: `ocamlc` is the bytecode
  compiler, `ocamlopt` the native one, and both ship in the same
  derivation. Pair with `dune` for the build tool and `opam` only if
  the project needs Hackage-style dependency resolution outside Nix;
  pure Nix consumers usually skip opam.

  Arguments:
  - `pkgs`: nixpkgs instance the compiler comes from.
  - `version`: required, one of `"latest" | "4.14" | "5.1" | "5.2"
    | "5.3" | "5.4"`. Pin `"4.14"` only for an upstream that has not
    migrated past the pre-multicore runtime; the long-term
    destination is the 5.x line.

  Example:
  ```nix
  { pkgs, ix, ... }:
  let
    ocaml = ix.languages.ocaml.compiler pkgs { version = "5.4"; };
    dune = ix.languages.ocaml.dune pkgs { version = "5.4"; };
  in {
    environment.systemPackages = [ ocaml dune ];
  }
  ```
  */
  compiler = pkgs: args: (packageSetFor pkgs (requireVersion "ix.languages.ocaml.compiler" args)).ocaml;

  /**
  Return Dune, the default OCaml build tool, from the matching
  package set so it shares its OCaml runtime with the `compiler`.

  Dune reads `dune-project` and `dune` files, drives `ocamlc` /
  `ocamlopt` underneath, and replaces hand-written Makefiles in
  nearly every modern OCaml project.
  */
  dune = pkgs: args: (packageSetFor pkgs (requireVersion "ix.languages.ocaml.dune" args)).dune_3;

  /**
  Return opam, the OCaml package manager. Resolves dependencies
  against the opam repository, which is the OCaml ecosystem's
  equivalent of Cargo or Hackage.

  Floating `pkgs.opam` because opam itself is not pinned to an
  OCaml version: it manages OCaml installations, it does not run
  inside one.
  */
  opam = pkgs: _: pkgs.opam;

  /**
  Return `ocamlformat`, the canonical OCaml formatter. Project
  formatting is configured via `.ocamlformat` in the project root.
  */
  ocamlformat = pkgs: _: pkgs.ocamlformat;

  /**
  Return `utop`, the modern OCaml REPL with completion and history.
  Built against the matching OCaml package set so loaded libraries
  are ABI-compatible.
  */
  utop = pkgs: args: (packageSetFor pkgs (requireVersion "ix.languages.ocaml.utop" args)).utop;

  /**
  Return `ocaml-lsp`, the OCaml language server, built against the
  matching OCaml package set. ocaml-lsp is compiler-version-coupled
  (it links against the `compiler-libs` of the OCaml it was built
  with), so pin both together.
  */
  languageServer = pkgs: args:
    (packageSetFor pkgs (requireVersion "ix.languages.ocaml.languageServer" args)).ocaml-lsp;
}
