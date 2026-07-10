_: {
  /**
  Return the Dhall interpreter.

  Dhall is a Haskell-implemented, total configuration language: every
  expression terminates, imports are pinned by SHA-256, and the
  standard distribution evaluates and type-checks rather than runs.
  Dhall and Nix overlap as config languages, so the value here is
  not "another way to write attrsets" but the totality guarantee and
  the hash-pinned import system: a Dhall import that resolves once
  cannot silently change shape later. Reach for it when an image
  needs to materialize JSON, YAML, Nix, or Bash from a typed
  description it can share with non-Nix consumers.

  `pkgs.dhall` provides the `dhall` CLI, which type-checks, hashes,
  normalizes, and freezes expressions. The sibling helpers below
  expose the conversion tools that share that core.

  Arguments:
  - `pkgs`: nixpkgs instance the binary comes from.

  Example:
  ```nix
  { pkgs, ix, ... }:
  let
    dhall = ix.languages.dhall.interpreter pkgs { };
    dhall-json = ix.languages.dhall.json pkgs { };
  in {
    environment.systemPackages = [ dhall dhall-json ];
  }
  ```
  */
  interpreter = pkgs: _: pkgs.dhall;

  /**
  Return `dhall-json`, which provides `dhall-to-json` and
  `dhall-to-yaml` (plus the reverse `json-to-dhall` / `yaml-to-dhall`
  importers).

  One package emits both JSON and YAML because the JSON pretty
  printer is the upstream substrate for the YAML writer; pulling the
  derivation in once is enough whether the consumer is a JSON API
  schema, a Kubernetes manifest, or anything else with a YAML
  encoding.
  */
  json = pkgs: _: pkgs.dhall-json;

  /**
  Return `dhall-nix`, which provides `dhall-to-nix` for emitting a
  Nix expression from a Dhall source.

  Useful when a generator wants to keep one Dhall source of truth and
  feed Nix from it; the reverse direction (`nix-to-dhall`) is not in
  this package because Nix is not total and the trip cannot be
  round-tripped in general.
  */
  nix = pkgs: _: pkgs.dhall-nix;

  /**
  Return `dhall-lsp-server`, the Dhall language server.

  Intended for dev VMs that host an editor; a sealed runtime image
  that only consumes the rendered JSON/YAML/Nix does not need it.
  */
  languageServer = pkgs: _: pkgs.dhall-lsp-server;
}
