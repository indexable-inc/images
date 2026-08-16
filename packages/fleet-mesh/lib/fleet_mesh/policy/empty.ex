defmodule FleetMesh.Policy.Empty do
  @moduledoc """
  The explicit no-conditions policy.

  For hosts without a catalog: tests, and public consumers of the engine.
  Naming this module in config is what distinguishes "this deployment wants
  no fleet conditions" from "someone forgot to wire the policy", which
  `FleetMesh.Policy.configured!/0` treats as a boot error.
  """

  @behaviour FleetMesh.Policy

  @impl true
  def conditions, do: []
end
