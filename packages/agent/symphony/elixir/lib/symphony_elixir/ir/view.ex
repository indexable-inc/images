defmodule SymphonyElixir.IR.View do
  @moduledoc """
  Render a `RunGraph` as plain JSON-able facts for the API and dashboard.

  This is a protocol emitter kept separate from the runtime: the runtime
  and `IR.*` modules produce facts (typed structs), and this module turns
  them into the canonical wire shape a consumer renders. Keeping it out of
  the runtime means a wire-format change never touches scheduling logic,
  and the same facts can feed an HTTP response, a LiveView, or a test
  assertion.

  The shapes are deliberately flat and string-keyed so `Jason.encode/1`
  handles them without a custom encoder. Tuples that only the interpreter
  understands (input refs, AST origins) are rendered as readable strings,
  not round-tripped: this is a read view, not the durable store (that is
  `IR.Store`, which preserves the exact terms).

  Two granularities:

  - `summary/1` is the list-row view: id, status, counts, cost total. Cheap
    enough to render for every run on an index.
  - `detail/1` is the single-run view: every node with its deps, attempts,
    and output, plus the expansion and audit logs.
  """

  alias SymphonyElixir.IR.Attempt
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.RunGraph

  @doc "Compact row for a run index: status and aggregate counts/cost."
  @spec summary(RunGraph.t()) :: map()
  def summary(%RunGraph{} = graph) do
    nodes = Map.values(graph.nodes)

    %{
      "run_id" => graph.run_id,
      "status" => Atom.to_string(graph.status),
      "trigger" => trigger_view(graph.trigger),
      "placement" => placement_view(graph.placement),
      "node_count" => length(nodes),
      "states" => state_counts(nodes),
      "cost_usd" => total_cost_usd(nodes),
      "created_at" => iso(graph.created_at),
      "updated_at" => iso(graph.updated_at)
    }
  end

  @doc """
  A human-readable label for a trigger map, shared between the summary view
  and the LiveView form so the same string appears in both places.
  """
  @spec trigger_label(map() | nil) :: String.t()
  def trigger_label(%{kind: :manual}), do: "manual"
  def trigger_label(%{kind: :cron, schedule: schedule}), do: "cron " <> to_string(schedule)
  def trigger_label(%{kind: :linear, label: label}), do: "linear: " <> to_string(label)
  def trigger_label(%{kind: :slack_huddle_completed, channel: c}), do: "huddle #" <> to_string(c)
  def trigger_label(%{kind: :slack_app_mention, channel: c}), do: "mention #" <> to_string(c)
  def trigger_label(%{kind: :github_pr_label, label: label}), do: "github: " <> to_string(label)
  def trigger_label(%{kind: kind}), do: to_string(kind)
  def trigger_label(_), do: "manual"

  @doc "Full run view: nodes with attempts and outputs, plus expansion and audit logs."
  @spec detail(RunGraph.t()) :: map()
  def detail(%RunGraph{} = graph) do
    graph
    |> summary()
    |> Map.merge(%{
      "nodes" => graph.nodes |> Map.values() |> Enum.sort_by(& &1.id) |> Enum.map(&render_node/1),
      "expansion_log" => Enum.map(graph.expansion_log, &expansion_event/1),
      "audit_log" => Enum.map(graph.audit_log, &audit_event/1)
    })
  end

  @doc "One node's facts: kind, state, deps, label, envelope, attempts, output."
  @spec render_node(Node.t()) :: map()
  def render_node(%Node{} = node) do
    %{
      "id" => node.id,
      "kind" => Atom.to_string(node.kind),
      "state" => Atom.to_string(node.state),
      "deps" => node.deps,
      "label" => node_label(node),
      "envelope" => envelope(node.envelope),
      "attempts" => Enum.map(node.attempts, &attempt/1),
      "output" => render_term(node.output),
      "updated_at" => iso(node.updated_at)
    }
  end

  # Derive a human-readable primary label for a node from its prompt_ref or
  # inputs. Agent nodes show the skill name (or "inline" for inline prompts).
  # Exec nodes show the script path from the resolved input literal. Other
  # kinds fall back to their kind string. This is the label the graph and
  # table surfaces use as the primary line; the node id is always available
  # separately as the secondary.
  defp node_label(%Node{kind: :agent, prompt_ref: {:skill, name, _}}), do: name
  defp node_label(%Node{kind: :agent, prompt_ref: {:inline, _}}), do: "inline"
  defp node_label(%Node{kind: :agent}), do: "agent"

  defp node_label(%Node{kind: :exec, inputs: inputs}) do
    case inputs["script"] do
      {:literal, script} when is_binary(script) -> script
      _ -> "exec"
    end
  end

  defp node_label(%Node{kind: kind}), do: Atom.to_string(kind)

  defp envelope(nil), do: nil

  defp envelope(%{engine: engine, model: model} = env) do
    %{
      "engine" => Atom.to_string(engine),
      "model" => model,
      "effort" => env.effort && Atom.to_string(env.effort),
      "permissions" => env.permissions && Atom.to_string(env.permissions),
      "location" => location(env.location)
    }
  end

  defp location(:local), do: "local"
  defp location(:ixvm), do: "ixvm"
  defp location({:host, name}), do: "host:#{name}"
  defp location({:room, url}), do: "room:#{url}"
  defp location(nil), do: nil

  defp attempt(%Attempt{} = attempt) do
    %{
      "n" => attempt.n,
      "engine" => Atom.to_string(attempt.engine),
      "state" => Atom.to_string(attempt.state),
      "thread_id" => attempt.thread_id,
      "outcome" => render_term(attempt.outcome),
      "cost" => cost(attempt.cost),
      "started_at" => iso(attempt.started_at),
      "finished_at" => iso(attempt.finished_at)
    }
  end

  defp cost(nil), do: nil
  defp cost(cost) when is_map(cost), do: Map.new(cost, fn {k, v} -> {Atom.to_string(k), v} end)

  defp audit_event(%{action: action} = event) do
    %{
      "action" => Atom.to_string(action),
      "target" => event.target,
      "actor" => render_term(event.actor),
      "detail" => render_term(event.detail),
      "at" => iso(event[:at])
    }
  end

  defp expansion_event(%{origin: origin, emitted: emitted} = event) do
    %{
      "origin" => render_term(origin),
      "observed" => render_term(event[:observed]),
      "emitted" => emitted,
      "at" => iso(event[:at])
    }
  end

  defp state_counts(nodes) do
    Enum.frequencies_by(nodes, fn node -> Atom.to_string(node.state) end)
  end

  # Sum the per-attempt usd cost across every node's every attempt. nil when
  # no attempt reported a cost, so the consumer can distinguish "free" from
  # "unknown".
  defp total_cost_usd(nodes) do
    costs =
      for node <- nodes,
          attempt <- node.attempts,
          is_map(attempt.cost),
          usd = attempt.cost[:usd],
          is_number(usd),
          do: usd

    case costs do
      [] -> nil
      _ -> Enum.sum(costs)
    end
  end

  # Render the trigger as a plain string label for the read view. Uses the
  # same label set as `trigger_label/1` so the API and the LiveView agree.
  defp trigger_view(nil), do: "manual"
  defp trigger_view(trigger), do: trigger_label(trigger)

  # Render the placement map for the read view. Exposes declared and
  # effective as strings so JSON consumers can distinguish a fallback
  # (declared: "ixvm", effective: "host") from a clean resolve.
  defp placement_view(nil), do: nil

  defp placement_view(%{declared: declared, effective: effective}) do
    %{
      "declared" => placement_location_string(declared),
      "effective" => if(effective, do: Atom.to_string(effective))
    }
  end

  defp placement_location_string(:local), do: "local"
  defp placement_location_string(:ixvm), do: "ixvm"
  defp placement_location_string({:host, name}), do: "host:#{name}"
  defp placement_location_string({:room, url}), do: "room:#{url}"
  defp placement_location_string(nil), do: nil

  defp iso(nil), do: nil
  defp iso(%DateTime{} = dt), do: DateTime.to_iso8601(dt)

  # A read view stringifies terms the interpreter owns (tuples, atoms,
  # nested refs) rather than round-tripping them. A plain JSON-able value
  # passes through so a node output map stays structured.
  defp render_term(nil), do: nil
  defp render_term(value) when is_binary(value) or is_number(value) or is_boolean(value), do: value
  defp render_term(value) when is_atom(value), do: Atom.to_string(value)

  defp render_term(value) when is_map(value) and not is_struct(value) do
    Map.new(value, fn {k, v} -> {render_key(k), render_term(v)} end)
  end

  defp render_term(value) when is_list(value), do: Enum.map(value, &render_term/1)
  defp render_term(value), do: inspect(value)

  defp render_key(k) when is_binary(k), do: k
  defp render_key(k) when is_atom(k), do: Atom.to_string(k)
  defp render_key(k), do: inspect(k)
end
