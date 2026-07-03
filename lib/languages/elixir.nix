{errors}: let
  /**
  Elixir minor → nixpkgs attribute mapping. Elixir is BEAM-hosted, so
  a "toolchain" here means the `elixir` package which bundles `elixir`,
  `elixirc`, `iex`, and `mix` against a chosen Erlang/OTP.

  `"latest"` follows whatever `pkgs.elixir` resolves to in the pinned
  nixpkgs (currently 1.19); the explicit minors are for builds that
  need to stay on a tested Elixir/OTP pairing.
  */
  toolchainsFor = pkgs: {
    "latest" = pkgs.elixir;
    "1.15" = pkgs.elixir_1_15;
    "1.16" = pkgs.elixir_1_16;
    "1.17" = pkgs.elixir_1_17;
    "1.18" = pkgs.elixir_1_18;
    "1.19" = pkgs.elixir_1_19;
  };
in {
  /**
  Return the Elixir toolchain for `version`.

  Elixir compiles to BEAM bytecode and runs on the Erlang VM that the
  nixpkgs `elixir` derivation pins. Selecting a specific minor here is
  the load-bearing knob: `mix.exs` files declare their Elixir version
  requirement and the build daemon refuses to load if the running
  Elixir does not match.

  Pair with [`ix.languages.erlang.toolchain`](./erlang.nix) when an
  image needs a specific Erlang/OTP version different from the one
  Elixir defaults to; otherwise the bundled OTP is the runtime.

  Arguments:
  - `pkgs`: nixpkgs instance the toolchain comes from.
  - `version`: required, one of `"latest" | "1.15" | "1.16" | "1.17"
    | "1.18" | "1.19"`. Pass `"latest"` to follow `pkgs.elixir`.

  Example:
  ```nix
  { pkgs, ix, ... }:
  let elixir = ix.languages.elixir.toolchain pkgs { version = "1.18"; };
  in { environment.systemPackages = [ elixir ]; }
  ```
  */
  toolchain = pkgs: args: let
    version = errors.requireArg {
      context = "ix.languages.elixir.toolchain";
      inherit args;
      name = "version";
    };
  in
    errors.requireAttr {
      context = "ix.languages.elixir.toolchain: unknown version";
      attrset = toolchainsFor pkgs;
      key = version;
    };

  /**
  Return the ElixirLS language server package.

  Intended for dev VMs that host an editor; runtime-only servers
  executing compiled BEAM `.beam`/`.ez` artifacts do not need it.
  */
  languageServer = pkgs: _: pkgs.elixir-ls;
}
