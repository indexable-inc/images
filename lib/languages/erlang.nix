{errors}: let
  /**
  Erlang/OTP major → nixpkgs attribute mapping.
  `pkgs.beamPackages.erlang` floats with whatever the channel ships;
  the OTP-numbered attributes are for builds that need to pin against
  a specific release line (distribution-protocol compatibility, BEAM
  JIT availability, `gen_statem` API changes between majors).

  Each major is spelled `beamNNPackages.erlang`, never the top-level
  `pkgs.erlang_NN`: those are `warnAlias` shims whose deprecation
  warning is fatal under `abort-on-warn` (see `elixir.nix`). OTP 26 is
  not listed at all -- nixpkgs removed it as EOL on 2026-04-01, so
  selecting it threw.
  */
  toolchainsFor = pkgs: {
    latest = pkgs.beamPackages.erlang;
    "27" = pkgs.beam27Packages.erlang;
    "28" = pkgs.beam28Packages.erlang;
    "29" = pkgs.beam29Packages.erlang;
  };
in {
  /**
  Return the Erlang/OTP toolchain for `version`.

  Bundles `erl`, `erlc`, the standard library, and the BEAM VM. The
  same derivation runs compiled `.beam` files at runtime and compiles
  new ones, which is why a single helper covers both build and
  runtime selection in this repo.

  Arguments:
  - `pkgs`: nixpkgs instance the toolchain comes from.
  - `version`: required, one of `"latest" | "27" | "28" | "29"`. Pass
    `"latest"` to follow `pkgs.beamPackages.erlang`.

  Example:
  ```nix
  { pkgs, ix, ... }:
  let erlang = ix.languages.erlang.toolchain pkgs { version = "27"; };
  in { environment.systemPackages = [ erlang ]; }
  ```
  */
  toolchain = pkgs: args: let
    version = errors.requireArg {
      context = "ix.languages.erlang.toolchain";
      inherit args;
      name = "version";
    };
  in
    errors.requireAttr {
      context = "ix.languages.erlang.toolchain: unknown version";
      attrset = toolchainsFor pkgs;
      key = version;
    };

  /**
  Return the rebar3 build tool, rebuilt against the chosen Erlang.

  rebar3 reads project metadata through the Erlang it runs on, so the
  two must match: the standard `pkgs.rebar3` derivation accepts an
  Erlang override via `overrideAttrs`, and devenv applies it because a
  mismatched runtime causes silent OTP-application start failures.

  Arguments:
  - `pkgs`: nixpkgs instance the rebar3 and Erlang packages come from.
  - `erlang`: optional resolved Erlang toolchain. Defaults to the same
    `pkgs.beamPackages.erlang` the `toolchain` helper returns for
    `version = "latest"`.
  */
  rebar3 = pkgs: {erlang ? pkgs.beamPackages.erlang}:
    pkgs.rebar3.overrideAttrs (_: {
      buildInputs = [erlang];
    });

  /**
  Return the Erlang language platform package (the
  EEF-supported successor to `erlang-ls`, which was archived upstream).

  Intended for dev VMs that host an editor; runtime-only BEAM servers
  do not need it.
  */
  languageServer = pkgs: _: pkgs.erlang-language-platform;
}
