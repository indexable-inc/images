defmodule IxMcp.Fleet.Topology do
  @moduledoc """
  What the BEAM mesh looks like right now, rendered for the `initialize`
  handshake.

  The node list is `IxMcp.Fleet.nodes/0` and nothing else: `IX_BEAM_NODES` is
  the fleet's one declaration of which hosts run a kernel, and a second
  inventory here would drift from it silently. What this module adds is
  liveness, which `Fleet.nodes/0` does not answer -- it parses an env var and
  never touches the network.

  Liveness has three states, not two. `:pang` from a node means the node is
  down; `:pang` from every node because distribution never started means we
  learned nothing about any of them. Reporting the second as "0 of 12
  reachable" would be a confident lie, so `summary/0` carries
  `distribution: {:error, reason}` and `render/1` says liveness is unknown.
  This matters today: on a host with no local epmd, `Fleet.ensure_dist/0`
  fails with `:nodistribution` and every ping is `:pang` regardless of fleet
  state (ENG-11209).
  """

  alias IxMcp.Fleet

  # Per-node ping budget. net_adm.ping blocks on TCP connect, so a firewalled
  # or dead host costs the full timeout; nodes are probed concurrently so the
  # whole sweep costs one timeout rather than one per host.
  @ping_timeout_ms 3_000

  @type state :: :up | :down | :unknown

  @type t :: %{
          configured: [node()],
          nodes: [{node(), state()}],
          distribution: :ok | {:error, term()},
          local: node()
        }

  @doc """
  Probe every configured node. Costs up to `#{@ping_timeout_ms}ms` total, and
  is called once per MCP connect rather than per tool call.
  """
  @spec summary() :: t()
  def summary do
    summary(Fleet.nodes())
  end

  # No nodes configured means no ping can tell us anything, so this must not
  # call ensure_dist/0: the answer is already known, and starting distribution
  # to discover it would add latency to every MCP handshake and break Fleet's
  # stated contract that distribution stays lazy so sandboxed builds never open
  # an epmd listen socket.
  defp summary([]),
    do: %{configured: [], nodes: [], distribution: :ok, local: Node.self()}

  defp summary(configured) do
    dist = Fleet.ensure_dist()

    %{
      configured: configured,
      nodes: probe(configured, dist),
      distribution: normalize(dist),
      local: Node.self()
    }
  end

  # With distribution down no ping can succeed, so probing would burn one
  # timeout per host to learn nothing. Mark every node unknown instead.
  defp probe(configured, {:error, _reason}), do: Enum.map(configured, &{&1, :unknown})

  defp probe(configured, _ok) do
    configured
    |> Task.async_stream(&{&1, ping(&1)},
      timeout: @ping_timeout_ms + 500,
      on_timeout: :kill_task,
      ordered: true
    )
    |> Enum.zip(configured)
    |> Enum.map(fn
      {{:ok, {node, state}}, _} -> {node, state}
      {{:exit, _reason}, node} -> {node, :unknown}
    end)
  end

  defp ping(node) do
    case :net_adm.ping(node) do
      :pong -> :up
      :pang -> :down
    end
  end

  defp normalize({:ok, _name}), do: :ok
  defp normalize({:error, reason}), do: {:error, reason}

  @doc """
  One paragraph naming every host the BEAM is on, for the operator to read
  once at connect. Kept short on purpose: this text is prepended to the
  server's `instructions`, which every session pays for in context.
  """
  @spec render(t()) :: String.t()
  def render(%{configured: []}) do
    "BEAM mesh: no nodes configured (IX_BEAM_NODES is unset), so no fleet host is reachable from this kernel."
  end

  def render(%{configured: configured, nodes: nodes, distribution: dist}) do
    up = for {node, :up} <- nodes, do: node
    down = for {node, :down} <- nodes, do: node
    total = length(configured)

    case dist do
      {:error, reason} ->
        """
        BEAM mesh: #{total} node(s) configured, liveness UNKNOWN -- Erlang \
        distribution did not start here (#{inspect_reason(reason)}), so no node \
        could be probed. This is a fault in this kernel's own networking, not a \
        statement about the fleet. Configured: #{short_list(configured)}.
        """
        |> String.trim()

      :ok ->
        """
        BEAM mesh: #{length(up)} of #{total} node(s) reachable.\
        #{if up != [], do: "\nUp: " <> short_list(up), else: ""}\
        #{if down != [], do: "\nUnreachable: " <> short_list(down), else: ""}
        """
        |> String.trim()
    end
  end

  # Node names are FQDNs on a tailnet, so the host part is the only
  # distinguishing text an operator reads; the domain is identical on all of
  # them and would trebl the length of the line for nothing.
  defp short_list(nodes) do
    Enum.map_join(nodes, ", ", fn node ->
      node |> Atom.to_string() |> String.split("@") |> List.last() |> String.split(".") |> hd()
    end)
  end

  defp inspect_reason(reason) do
    case reason do
      {{:shutdown, {:failed_to_start_child, :net_kernel, _}}, _} ->
        "net_kernel would not start; usually no local epmd"

      other ->
        other |> inspect() |> String.slice(0, 120)
    end
  end
end
