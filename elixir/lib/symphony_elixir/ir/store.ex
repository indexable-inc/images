defmodule SymphonyElixir.IR.Store do
  @moduledoc """
  Durable persistence of a `RunGraph` to disk as JSON, one file per run.
  Atomic temp-then-rename writes so a crash mid-write never leaves a
  half-written run file, and a tolerant loader that quarantines a corrupt
  file rather than crashing boot.

  Disk layout, under `runs_dir/ir/`:

      runs/
        ir/
          <run_id>.json       one RunGraph, full state
          <run_id>.json.bad   a file that failed to decode, quarantined

  This is plain serialization. It holds no process state; `Runtime` calls
  `persist/1` after every transition. The directory is taken from
  `Config.get().runs_dir` by default and can be overridden with `dir:` so
  tests isolate to a tmp dir without booting Config.

  The round-trip target is the full `RunGraph`: nodes (with envelope,
  prompt_ref, inputs, deps, attempts, output), the run status, and the
  append-only `expansion_log`. Recovery (`Runtime.reconcile/1`) depends on
  that round-trip being faithful, so the encode/decode pair here is the
  contract restart correctness rests on.
  """

  require Logger

  alias SymphonyElixir.Config
  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.{Attempt, Node, RunGraph}

  @doc "Directory holding IR run files. Defaults to `Config.get().runs_dir/ir`."
  @spec dir(keyword()) :: Path.t()
  def dir(opts \\ []) do
    case Keyword.get(opts, :dir) do
      nil -> Path.join(Config.get().runs_dir, "ir")
      explicit -> explicit
    end
  end

  @doc """
  Load every decodable RunGraph from the store directory. A file that
  fails to decode is renamed to `<name>.bad` and skipped, with a warning,
  so one corrupt run never blocks boot. Returns the graphs that loaded.
  """
  @spec load_all(keyword()) :: [RunGraph.t()]
  def load_all(opts \\ []) do
    store_dir = dir(opts)
    File.mkdir_p!(store_dir)

    store_dir
    |> Path.join("*.json")
    |> Path.wildcard()
    |> Enum.flat_map(fn path ->
      case read(path) do
        {:ok, graph} ->
          [graph]

        {:error, reason} ->
          quarantine(path, reason)
          []
      end
    end)
  end

  @doc "Load one RunGraph by run id. `{:ok, graph}` or `{:error, :not_found}` / decode error."
  @spec load(String.t(), keyword()) :: {:ok, RunGraph.t()} | {:error, term()}
  def load(run_id, opts \\ []) when is_binary(run_id) do
    path = run_path(dir(opts), run_id)

    if File.exists?(path) do
      read(path)
    else
      {:error, :not_found}
    end
  end

  @doc "Persist a RunGraph, atomically replacing any prior file for the run."
  @spec persist(RunGraph.t(), keyword()) :: :ok | {:error, term()}
  def persist(%RunGraph{} = graph, opts \\ []) do
    store_dir = dir(opts)
    File.mkdir_p!(store_dir)
    path = run_path(store_dir, graph.run_id)
    tmp = path <> ".tmp"

    with {:ok, encoded} <- Jason.encode(encode(graph), pretty: true),
         :ok <- File.write(tmp, encoded),
         :ok <- File.rename(tmp, path) do
      :ok
    end
  end

  @doc """
  Append a dynamic-expansion event and persist. A thin wrapper over
  `RunGraph.append_expansion/4` plus `persist/2`, so the append-only log
  that drives replay is never updated without hitting disk.
  """
  @spec append_expansion(RunGraph.t(), {term(), term(), [String.t()]}, keyword()) ::
          {:ok, RunGraph.t()} | {:error, term()}
  def append_expansion(%RunGraph{} = graph, {origin, observed, emitted}, opts \\ []) do
    next = RunGraph.append_expansion(graph, origin, observed, emitted)

    case persist(next, opts) do
      :ok -> {:ok, next}
      error -> error
    end
  end

  @doc "Delete a run's file. Returns `:ok` even when the file was already gone."
  @spec delete(String.t(), keyword()) :: :ok
  def delete(run_id, opts \\ []) when is_binary(run_id) do
    _ = File.rm(run_path(dir(opts), run_id))
    :ok
  end

  defp run_path(store_dir, run_id), do: Path.join(store_dir, run_id <> ".json")

  defp read(path) do
    with {:ok, raw} <- File.read(path),
         {:ok, decoded} <- Jason.decode(raw),
         {:ok, graph} <- decode(decoded) do
      {:ok, graph}
    end
  rescue
    # Any raise inside decode (an enum string outside its set, a corrupt
    # timestamp, an undecodable term blob) becomes a quarantine reason
    # rather than a boot crash, keeping the load_all promise that one bad
    # file never blocks startup.
    error -> {:error, {:decode_crashed, error}}
  end

  defp quarantine(path, reason) do
    bad = path <> ".bad"
    _ = File.rename(path, bad)
    Logger.warning("IR.Store quarantined #{path} (#{inspect(reason)}) -> #{bad}")
  end

  # --- encode ---------------------------------------------------------

  defp encode(%RunGraph{} = graph) do
    %{
      "run_id" => graph.run_id,
      "source_hash" => Base.encode16(graph.source_hash, case: :lower),
      "ast" => encode_term(graph.ast),
      "trigger" => encode_term(graph.trigger),
      "status" => Atom.to_string(graph.status),
      "placement" => encode_placement(graph.placement),
      "nodes" => Map.new(graph.nodes, fn {id, node} -> {id, encode_node(node)} end),
      "expansion_log" => Enum.map(graph.expansion_log, &encode_expansion/1),
      "audit_log" => Enum.map(graph.audit_log, &encode_audit/1),
      "created_at" => encode_dt(graph.created_at),
      "updated_at" => encode_dt(graph.updated_at)
    }
  end

  defp encode_placement(nil), do: nil

  defp encode_placement(%{declared: declared, effective: effective}) do
    %{
      "declared" => encode_placement_location(declared),
      "effective" => if(effective, do: Atom.to_string(effective), else: nil)
    }
  end

  # Placement declared is an Envelope location, which can be :ixvm, :local,
  # {:host, name}, {:room, url}, or nil. Encode using the same shape as
  # encode_location/1 in the envelope path so the wire format is consistent.
  defp encode_placement_location(:local), do: "local"
  defp encode_placement_location(:ixvm), do: "ixvm"
  defp encode_placement_location({:host, name}), do: %{"host" => name}
  defp encode_placement_location({:room, url}), do: %{"room" => url}
  defp encode_placement_location(nil), do: nil

  defp encode_node(%Node{} = node) do
    %{
      "id" => node.id,
      "ast_origin" => encode_term(node.ast_origin),
      "kind" => Atom.to_string(node.kind),
      "envelope" => encode_envelope(node.envelope),
      "prompt_ref" => encode_term(node.prompt_ref),
      "inputs" => encode_inputs(node.inputs),
      "deps" => node.deps,
      "expansion_key" => encode_term(node.expansion_key),
      "state" => Atom.to_string(node.state),
      "output" => encode_term(node.output),
      "attempts" => Enum.map(node.attempts, &encode_attempt/1),
      "created_at" => encode_dt(node.created_at),
      "updated_at" => encode_dt(node.updated_at)
    }
  end

  defp encode_envelope(nil), do: nil

  defp encode_envelope(%Envelope{} = env) do
    %{
      "engine" => Atom.to_string(env.engine),
      "model" => env.model,
      "effort" => env.effort && Atom.to_string(env.effort),
      "permissions" => env.permissions && Atom.to_string(env.permissions),
      "location" => encode_location(env.location)
    }
  end

  defp encode_location(:local), do: "local"
  defp encode_location(:ixvm), do: "ixvm"
  defp encode_location({:host, name}), do: %{"host" => name}
  defp encode_location({:room, url}), do: %{"room" => url}
  defp encode_location(nil), do: nil

  defp encode_attempt(%Attempt{} = attempt) do
    %{
      "n" => attempt.n,
      "engine" => Atom.to_string(attempt.engine),
      "thread_id" => attempt.thread_id,
      "state" => Atom.to_string(attempt.state),
      "started_at" => encode_dt(attempt.started_at),
      "finished_at" => encode_dt(attempt.finished_at),
      "outcome" => encode_term(attempt.outcome),
      "cost" => encode_cost(attempt.cost),
      "events_ref" => attempt.events_ref
    }
  end

  defp encode_cost(nil), do: nil
  defp encode_cost(cost) when is_map(cost), do: Map.new(cost, fn {k, v} -> {Atom.to_string(k), v} end)

  # Inputs and AST fragments carry tuples (`{:node, id, path}`,
  # `{:literal, value}`) that JSON cannot represent. Encode through the
  # Erlang external term format and Base64 so the decode side reconstructs
  # the exact term, including atoms, tuples, and nested structures. This
  # is the same round-trip guarantee `:erlang.term_to_binary/1` gives,
  # chosen over a bespoke tuple-to-list scheme because the AST shape is
  # owned by the interpreter (WS-1) and must survive verbatim for replay.
  defp encode_inputs(inputs) when is_map(inputs) do
    Map.new(inputs, fn {key, ref} -> {key, encode_term(ref)} end)
  end

  defp encode_term(nil), do: nil
  defp encode_term(term), do: %{"__term__" => Base.encode64(:erlang.term_to_binary(term))}

  defp encode_dt(nil), do: nil
  defp encode_dt(%DateTime{} = dt), do: DateTime.to_iso8601(dt)

  # --- decode ---------------------------------------------------------

  defp decode(%{"run_id" => run_id, "source_hash" => source_hash_hex, "status" => status, "nodes" => nodes} = payload) do
    with {:ok, source_hash} <- Base.decode16(source_hash_hex, case: :lower),
         {:ok, decoded_nodes} <- decode_nodes(nodes) do
      {:ok,
       %RunGraph{
         run_id: run_id,
         source_hash: source_hash,
         ast: decode_term(payload["ast"]),
         trigger: decode_term(payload["trigger"]),
         status: known_atom(status, RunGraph.statuses(), "run status"),
         placement: decode_placement(payload["placement"]),
         nodes: decoded_nodes,
         expansion_log: Enum.map(payload["expansion_log"] || [], &decode_expansion/1),
         audit_log: Enum.map(payload["audit_log"] || [], &decode_audit/1),
         created_at: decode_dt(payload["created_at"]),
         updated_at: decode_dt(payload["updated_at"])
       }}
    else
      :error -> {:error, :invalid_source_hash}
      other -> other
    end
  end

  defp decode(_), do: {:error, :invalid_run_graph_payload}

  # Valid effective locations that may appear in a persisted placement.
  # Guards against a tampered file minting atoms outside the known set.
  # `:remote` is the effective location when an `:ixvm` run falls back to a
  # runtime worker; omitting it quarantines every successful remote run.
  @effective_locations [:ixvm, :host, :remote, :local]

  defp decode_placement(nil), do: nil

  defp decode_placement(%{"declared" => declared, "effective" => effective}) do
    %{
      declared: decode_placement_location(declared),
      effective: known_atom_or_nil(effective, @effective_locations, "effective placement location")
    }
  end

  defp decode_placement(_), do: nil

  # Mirrors encode_placement_location/1; decodes the same shapes the
  # envelope location decoder handles.
  defp decode_placement_location("local"), do: :local
  defp decode_placement_location("ixvm"), do: :ixvm
  defp decode_placement_location(%{"host" => name}), do: {:host, name}
  defp decode_placement_location(%{"room" => url}), do: {:room, url}
  defp decode_placement_location(nil), do: nil

  defp decode_nodes(nodes) when is_map(nodes) do
    decoded =
      Map.new(nodes, fn {id, node_payload} ->
        {id, decode_node(node_payload)}
      end)

    {:ok, decoded}
  rescue
    error -> {:error, {:invalid_node, error}}
  end

  defp decode_nodes(_), do: {:error, :invalid_nodes}

  defp decode_node(%{"id" => id, "kind" => kind, "state" => state} = payload) do
    %Node{
      id: id,
      ast_origin: decode_term(payload["ast_origin"]),
      kind: known_atom(kind, Node.kinds(), "node kind"),
      envelope: decode_envelope(payload["envelope"]),
      prompt_ref: decode_term(payload["prompt_ref"]),
      inputs: decode_inputs(payload["inputs"]),
      deps: payload["deps"] || [],
      expansion_key: decode_term(payload["expansion_key"]),
      state: known_atom(state, Node.states(), "node state"),
      output: decode_term(payload["output"]),
      attempts: Enum.map(payload["attempts"] || [], &decode_attempt/1),
      created_at: decode_dt(payload["created_at"]),
      updated_at: decode_dt(payload["updated_at"])
    }
  end

  defp decode_envelope(nil), do: nil

  defp decode_envelope(%{"engine" => engine, "model" => model} = payload) do
    %Envelope{
      engine: known_atom(engine, Envelope.engines(), "engine"),
      model: model,
      effort: known_atom_or_nil(payload["effort"], Envelope.efforts(), "effort"),
      permissions: known_atom_or_nil(payload["permissions"], Envelope.permission_levels(), "permissions"),
      location: decode_location(payload["location"])
    }
  end

  defp decode_location("local"), do: :local
  defp decode_location("ixvm"), do: :ixvm
  defp decode_location(%{"host" => name}), do: {:host, name}
  defp decode_location(%{"room" => url}), do: {:room, url}
  defp decode_location(nil), do: nil

  defp decode_attempt(%{"n" => n, "engine" => engine, "state" => state} = payload) do
    %Attempt{
      n: n,
      engine: known_atom(engine, Attempt.engines(), "attempt engine"),
      thread_id: payload["thread_id"],
      state: known_atom(state, Attempt.states(), "attempt state"),
      started_at: decode_dt(payload["started_at"]),
      finished_at: decode_dt(payload["finished_at"]),
      outcome: decode_term(payload["outcome"]),
      cost: decode_cost(payload["cost"]),
      events_ref: payload["events_ref"]
    }
  end

  @cost_keys [:usd, :tokens_in, :tokens_out, :cache_read, :cache_creation]

  defp decode_cost(nil), do: nil

  defp decode_cost(cost) when is_map(cost) do
    Map.new(cost, fn {k, v} -> {known_atom(k, @cost_keys, "cost key"), v} end)
  end

  defp decode_inputs(nil), do: %{}
  defp decode_inputs(inputs) when is_map(inputs), do: Map.new(inputs, fn {key, ref} -> {key, decode_term(ref)} end)

  # Decode an enum string against its owning module's set. An unknown value
  # raises (caught by read/1 and quarantined) with an actionable reason,
  # instead of String.to_existing_atom's opaque ArgumentError or accepting
  # an unrelated-but-existing atom. Only matches against atoms that already
  # exist as module attributes, so it cannot grow the atom table.
  defp known_atom(value, allowed, context) when is_binary(value) do
    case Enum.find(allowed, fn atom -> Atom.to_string(atom) == value end) do
      nil -> raise ArgumentError, "invalid #{context} #{inspect(value)} in run graph"
      atom -> atom
    end
  end

  defp known_atom(value, _allowed, context) do
    raise ArgumentError, "invalid #{context} #{inspect(value)} in run graph"
  end

  defp known_atom_or_nil(nil, _allowed, _context), do: nil
  defp known_atom_or_nil(value, allowed, context), do: known_atom(value, allowed, context)

  defp decode_term(nil), do: nil

  defp decode_term(%{"__term__" => encoded}) when is_binary(encoded) do
    encoded
    |> Base.decode64!()
    # These terms are symphony's own data: it wrote them with
    # `:erlang.term_to_binary/1`, so every atom inside existed in this app at
    # write time (AST tags, node kinds, error reasons like `:missing_cwd`).
    # `:safe` would refuse to recreate any of those atoms that are not yet
    # interned when a run is reloaded (e.g. a failure-path reason atom after a
    # fresh boot), which crashed decode and quarantined otherwise-valid runs.
    # The store is root-owned local state under /var/lib/symphony; an attacker
    # who can rewrite it already owns the host, so `:safe`'s tamper guard buys
    # nothing here while costing run durability.
    |> :erlang.binary_to_term()
  end

  defp encode_expansion(%{origin: origin, observed: observed, emitted: emitted, at: at}) do
    %{
      "origin" => encode_term(origin),
      "observed" => encode_term(observed),
      "emitted" => emitted,
      "at" => encode_dt(at)
    }
  end

  defp decode_expansion(%{"origin" => origin, "observed" => observed, "emitted" => emitted} = payload) do
    %{
      origin: decode_term(origin),
      observed: decode_term(observed),
      emitted: emitted || [],
      at: decode_dt(payload["at"])
    }
  end

  # `actor` and `detail` carry arbitrary terms (operator ids, tuples), so
  # they round-trip through the same term encoding the inputs use; `action`
  # is a known operator-action atom.
  defp encode_audit(%{action: action, target: target, actor: actor, detail: detail, at: at}) do
    %{
      "action" => Atom.to_string(action),
      "target" => target,
      "actor" => encode_term(actor),
      "detail" => encode_term(detail),
      "at" => encode_dt(at)
    }
  end

  defp decode_audit(%{"action" => action} = payload) do
    %{
      action: known_audit_action(action),
      target: payload["target"],
      actor: decode_term(payload["actor"]),
      detail: decode_term(payload["detail"]),
      at: decode_dt(payload["at"])
    }
  end

  @audit_actions ~w(cancel retry_node rerun clear_failed)a

  # Map a serialized action back to a known atom. An unknown string is
  # kept as a string rather than minting an atom, so a tampered or
  # future-version file cannot exhaust the atom table on load.
  defp known_audit_action(action) when is_binary(action) do
    Enum.find(@audit_actions, action, fn a -> Atom.to_string(a) == action end)
  end

  defp decode_dt(nil), do: nil

  defp decode_dt(iso) when is_binary(iso) do
    case DateTime.from_iso8601(iso) do
      {:ok, dt, _} ->
        dt

      # A corrupt timestamp is a decode failure (caught by read/1 and
      # quarantined), not a silent drop of audit-trail metadata to nil.
      _ ->
        raise ArgumentError, "invalid ISO8601 datetime #{inspect(iso)} in run graph"
    end
  end
end
