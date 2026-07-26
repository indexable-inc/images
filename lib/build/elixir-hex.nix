{
  /**
  `hex` built against a given Elixir, with the darwin sandbox allowance Mix
  needs from 1.19 onward.

  Mix 1.19 added `Mix.PubSub`, which `Mix.Sync.PubSub` implements by binding a
  TCP socket on 127.0.0.1 at COMPILE time to coordinate concurrent OS
  processes. There is no env var to turn it off -- `mix/sync/pubsub.ex` binds
  `@loopback {127, 0, 0, 1}` unconditionally -- so under the darwin sandbox
  every `mix` invocation aborts before it does any work:

      ** (RuntimeError) failed to start Mix.PubSub, reason: ... failed to open
      a TCP socket in Mix.Sync.PubSub.subscribe/1, reason: :eperm

  nixpkgs' `beamPackages.hex` does not set `__darwinAllowLocalNetworking`, and
  hex is itself compiled with mix, so it is the first thing to die. The Linux
  sandbox permits the bind, which is why this is invisible in CI and fails only
  on a developer Mac.

  Three call sites wanted the identical `beamPackages.hex.override {inherit
  elixir;}`; they now share this so the allowance cannot be added to two of
  them and forgotten on the third.

  Upstream fix belongs in nixpkgs (`beamPackages.buildMix` / `hex` should carry
  the allowance for Elixir >= 1.19); carry this until it lands.
  */
  pkgs,
  elixir,
}:
(pkgs.beamPackages.hex.override {inherit elixir;}).overrideAttrs (_: {
  __darwinAllowLocalNetworking = true;
})
