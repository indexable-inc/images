defmodule IxMcp.FleetTopologyRenderTest do
  @moduledoc """
  Topology rendering: liveness unknown is never spelled as zero reachable.
  Moved intact from the retired fleet_alerts_test when the catalog went
  private; these never touched the catalog.
  """

  use ExUnit.Case, async: true

  alias IxMcp.Fleet.Topology

  test "distribution being down is reported as unknown, not as zero reachable" do
    rendered =
      Topology.render(%{
        configured: [:"beamd@a.example", :"beamd@b.example"],
        nodes: [{:"beamd@a.example", :unknown}, {:"beamd@b.example", :unknown}],
        distribution: {:error, :nodistribution},
        local: :nonode@nohost
      })

    assert rendered =~ "liveness UNKNOWN"
    assert rendered =~ "2 node(s) configured"
    refute rendered =~ "0 of 2"
    assert rendered =~ "a, b"
  end

  test "a working mesh reports which hosts are up and which are not" do
    rendered =
      Topology.render(%{
        configured: [:"beamd@a.example", :"beamd@b.example"],
        nodes: [{:"beamd@a.example", :up}, {:"beamd@b.example", :down}],
        distribution: :ok,
        local: :mcp@here
      })

    assert rendered =~ "1 of 2 node(s) reachable"
    assert rendered =~ "Up: a"
    assert rendered =~ "Unreachable: b"
  end

  test "no configured nodes says so plainly" do
    rendered =
      Topology.render(%{configured: [], nodes: [], distribution: :ok, local: :nonode@nohost})

    assert rendered =~ "no nodes configured"
    assert rendered =~ "IX_BEAM_NODES"
  end
end
