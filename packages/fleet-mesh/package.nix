# Registry metadata. fleet-mesh is the public half of the fleet warning
# system: a Condition/Policy/Engine trio (conditions as data, policy as a
# behaviour, engine emitting one snapshot then edges) plus the BEAM mesh
# client (IX_BEAM_NODES/IX_BEAM_COOKIE contract, :erpc dispatch) that
# ix-mcp-ex and the test-ide dashboard previously each kept a copy of. It
# carries no thresholds, host names or measured rates; those live in the
# private policy package. ix-mcp-ex consumes it as a mix path dependency;
# the flake output builds the compiled :fleet_mesh OTP app and
# `passthru.tests.elixir` gates the ExUnit/Credo lane.
{
  id = "fleet-mesh";
  packageSet = true;
  flake = true;
  overlay = false;
  passthruTests = true;
}
