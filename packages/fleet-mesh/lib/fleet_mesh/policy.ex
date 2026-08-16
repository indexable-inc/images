defmodule FleetMesh.Policy do
  @moduledoc """
  The seam between this public engine and whatever private catalog drives it.

  This package is publicly projected, so it must not know a single threshold,
  host name or measured rate. A policy module owns all of those: it builds
  `FleetMesh.Condition` structs and hands them over through the one callback
  here. The engine reads which module that is from application config:

      config :fleet_mesh, policy: MyPolicy

  `configured!/0` raises when nothing is configured, at boot, while someone
  is looking. A default would be worse than the crash: an engine silently
  running an empty catalog reports a healthy fleet it never measured. The
  explicit `FleetMesh.Policy.Empty` exists for hosts that genuinely want no
  conditions (tests, public consumers without a catalog), and choosing it is
  a line in their config rather than an accident.
  """

  @doc "Every condition the engine should evaluate."
  @callback conditions() :: [FleetMesh.Condition.t()]

  @doc """
  The policy module named in `config :fleet_mesh, :policy`. Raises when the
  key is absent; see the moduledoc for why there is no default.
  """
  @spec configured!() :: module()
  def configured! do
    Application.fetch_env!(:fleet_mesh, :policy)
  end
end
