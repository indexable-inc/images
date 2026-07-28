{errors}: let
  /**
  Elixir minor → nixpkgs attribute mapping. Elixir is BEAM-hosted, so
  a "toolchain" here means the `elixir` package which bundles `elixir`,
  `elixirc`, `iex`, and `mix` against a chosen Erlang/OTP.

  `"latest"` follows whatever `pkgs.beamPackages.elixir` resolves to in
  the pinned nixpkgs (currently 1.18); the explicit minors are for builds
  that need to stay on a tested Elixir/OTP pairing.

  Every entry is spelled `beamPackages.*`, never the top-level
  `pkgs.elixir_1_*`. Nixpkgs turned those into `warnAlias` shims on
  2026-06-15, and `lib.derivations.warnOnInstantiate` wraps every
  attribute of the derivation except `meta`/`name`/`type`/`outputName`
  in `lib.warn` -- so merely forcing `drvPath` emits "'elixir_1_19' is
  deprecated in favor of using the beamPackages sets", which
  `abort-on-warn` (set here and in ix) turns into a hard eval failure.
  Only versions that run on the `beamPackages` OTP are listed. 1.15 and 1.16
  throw outright (nixpkgs removed them on 2026-04-01 with `erlang_26` as EOL),
  and 1.17 asserts `OTP >= 25 and <= 27` so it cannot instantiate against the
  OTP 28 that `beamPackages` now tracks. Reviving 1.17 would mean pinning
  `beam27Packages`; nothing asked for it, so it is gone.
  */
  toolchainsFor = pkgs: {
    latest = pkgs.beamPackages.elixir;
    "1.18" = pkgs.beamPackages.elixir_1_18;
    "1.19" = pkgs.beamPackages.elixir_1_19;
    "1.20" = pkgs.beamPackages.elixir_1_20;
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
  - `version`: required, one of `"latest" | "1.18" | "1.19" | "1.20"`.
    Pass `"latest"` to follow `pkgs.beamPackages.elixir`.

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
