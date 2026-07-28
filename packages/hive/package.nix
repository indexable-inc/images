# Registry metadata. `hive` is an experimental personal package: a tiny Elixir
# agent mesh used to exercise the repo's Elixir type-discipline gate (the
# `elixir.astlog` rules and the `mix compile --warnings-as-errors` check wired
# in lib/per-system.nix). It is a flake output so `nix run .#hive` runs the demo.
{
  id = "hive";
  packageSet = true;
  flake = true;
  overlay = false;
}
