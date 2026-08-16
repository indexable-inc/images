# fleet-mesh

The public half of the fleet warning system, and the one copy of the BEAM
mesh client.

- `FleetMesh.Condition`: one thing worth knowing about the fleet, as data.
  States are `:green | :red | :unknown`; a check that could not run is
  `:unknown`, never silently green.
- `FleetMesh.Policy`: the seam. A private module implements `conditions/0`
  and is named in `config :fleet_mesh, policy: ...`. This package carries no
  thresholds, host names or measured rates.
- `FleetMesh.Engine`: evaluates conditions on their intervals. `snapshot/1`
  for "where the fleet stands now"; `subscribe/2` delivers one snapshot then
  edges only, and reports existing subscribers so watches stay deduplicated.
- `FleetMesh.Mesh`: `:erpc` dispatch over Erlang distribution, configured by
  `IX_BEAM_NODES` and `IX_BEAM_COOKIE`.
