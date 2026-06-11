_: {
  /**
    Return the Gleam compiler.

    Gleam is a statically-typed functional language whose compiler is
    written in Rust and which targets BEAM (Erlang's VM) or JavaScript.
    The runtime story follows the chosen target: on BEAM, pair with
    [`ix.languages.erlang.toolchain`](./erlang.nix) so `gleam run`
    finds an `erl` it can hand the compiled `.beam` files to; on the
    JS target, pair with [`ix.languages.javascript.node`](./javascript.nix)
    (or `bun` / `deno`) for the runtime.

    `pkgs.gleam` is one binary that covers the whole workflow: `gleam
    build`, `gleam test`, `gleam shell`, and `gleam lsp` (the language
    server is built in — there is no separate `gleam-language-server`
    package). The version helper that the rest of `ix.languages` uses
    is omitted here because nixpkgs ships a single floating `pkgs.gleam`
    and Gleam moves fast enough that an explicit per-minor table would
    be stale within a channel bump; override at the call site if a
    project genuinely needs to pin.

    Arguments:
    - `pkgs`: nixpkgs instance the compiler comes from.

    Example:
    ```nix
    { pkgs, ix, ... }:
    let
      gleam = ix.languages.gleam.compiler pkgs { };
      erlang = ix.languages.erlang.toolchain pkgs { };
    in {
      environment.systemPackages = [ gleam erlang ];
    }
    ```
  */
  compiler =
    pkgs: _:
    # gleam 1.17.0's tests::escript_success_with_dependency fetches a hex
    # dependency, so the package fails to build in the Nix sandbox on pins
    # that predate the upstream fix (nixpkgs d74ffb2d2d "gleam: fix linux
    # build by skipping network test"). Mirror that fix here so fleet nodes
    # build against any consumer's nixpkgs pin; drop once the consumed pins
    # include it.
    pkgs.gleam.overrideAttrs (old: {
      checkFlags = (old.checkFlags or [ ]) ++ [ "--skip=tests::escript_success_with_dependency" ];
    });
}
