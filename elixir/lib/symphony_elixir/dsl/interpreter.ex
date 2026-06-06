defmodule SymphonyElixir.DSL.Interpreter do
  @moduledoc """
  Expand a reified `SymphonyElixir.DSL.AST` into a delta of durable
  `SymphonyElixir.IR.Node`s. This is the eval step of eval-as-emission and
  the deepest module in the DSL front end.

      expand(ast, known_outputs, expansion_log) ->
        {ir_delta, pending, new_log}

  - `ir_delta` is the list of `IR.Node` structs to materialize on this
    pass. Re-running `expand` with more `known_outputs` yields the next
    delta; the runtime folds each delta into the `RunGraph`.
  - `pending` is the set of unresolved AST points still awaiting inputs,
    as plain data: `{:awaiting, ast_node_id, needed_node_ids}`. The
    runtime uses it to know an effect cannot be materialized until those
    upstream node outputs arrive.
  - `new_log` is the `expansion_log` extended with one event per dynamic
    expansion that fired on this pass (a `when` that opened, an
    `every_nth` that fired, a `map` that fanned out). The order is
    load-bearing for deterministic replay.

  ## Rules (from the overhaul plan, Pillar 1)

  1. Only effectful constructors (`agent`, `exec`, `subrun`) become IR
     nodes. Pure computation (string concat, `${session.area}` field
     access) is evaluated inside the interpreter at expand time and never
     fills the graph with trivial nodes.
  2. `IR.Node.deps` is derived from `inputs` via
     `IR.Node.deps_from_inputs/1`. A `{:node, id, path}` input is the only
     thing that makes an edge; the interpreter never hand-writes `deps`.
  3. Dynamic constructs (`when`, `every_nth`, `map`) emit a placeholder
     node (`:gate` or `:map_fanout`) when their gating input is
     unresolved, and emit children deterministically when re-expanded with
     the resolved output.
  4. Determinism: gates are pure functions of `known_outputs` and the
     persisted counters recovered from `expansion_log`. No wall clock, no
     RNG. The same `(ast, known_outputs, expansion_log)` always yields the
     same `ir_delta`, which is the invariant the runtime replays on
     restart.

  ## Input resolution

  Each input value reduces to a single `IR.Node.input_ref`:

  - A fully-literal pure value folds to `{:literal, computed}`.
  - A single `${node.path}` read of an effect binding becomes
    `{:node, id, path}`, the one shape that creates a dependency edge.
  - A `concat`/`list` that mixes literals with a node read cannot be one
    ref. The effect carrying it is deferred (reported in `pending`) until
    every referenced node output is in `known_outputs`, then the value
    folds to a literal on re-expansion.
  """

  alias SymphonyElixir.DSL.AST
  alias SymphonyElixir.IR.Node

  @type known_outputs :: %{optional(String.t()) => term()}
  @type pending :: {:awaiting, String.t(), [String.t()]}
  @type expansion_log :: [map()]

  @type result :: {[Node.t()], [pending()], expansion_log()}

  @doc """
  Expand the workflow against the outputs known so far and the prior
  expansion log. Pure; same inputs always produce the same result.
  """
  @spec expand(AST.workflow(), known_outputs(), expansion_log()) :: result()
  def expand(%{kind: :workflow, statements: statements}, known_outputs \\ %{}, expansion_log \\ [])
      when is_map(known_outputs) and is_list(expansion_log) do
    counters = counters_from_log(expansion_log)

    acc = %{
      env: %{},
      nodes: [],
      pending: [],
      log: [],
      counters: counters,
      prior_ticks: prior_ticks_from_log(expansion_log),
      known: known_outputs
    }

    final = Enum.reduce(statements, acc, &expand_statement/2)

    {Enum.reverse(final.nodes), Enum.reverse(final.pending), expansion_log ++ Enum.reverse(final.log)}
  end

  # --- statements ---------------------------------------------------------

  # A `let` binds a pure value. It is computed now when its dependencies
  # are resolvable and recorded as a value binding; otherwise it is bound
  # to whatever node ref it reads so later effects still see the edge.
  defp expand_statement({:let, name, pure}, acc) do
    case resolve_value(pure, acc.env, acc.known) do
      {:value, value} -> put_env(acc, name, {:value, value})
      {:node, id, path} -> put_env(acc, name, {:node_field, id, path})
      :deferred -> put_env(acc, name, {:deferred, pure})
    end
  end

  defp expand_statement({:bind, name, expr}, acc) do
    expand_effect(expr, name, acc)
  end

  defp expand_statement(expr, acc) when is_map(expr) do
    expand_effect(expr, nil, acc)
  end

  # --- effects ------------------------------------------------------------

  defp expand_effect(%{kind: :agent} = agent, bind_name, acc) do
    expansion_key = nil
    id = node_id(agent.id, expansion_key)

    {prompt_ref, prompt_inputs} = resolve_prompt(agent.prompt, agent.id, acc)
    {explicit_inputs, explicit_pending} = resolve_inputs(agent.inputs, agent.id, acc)
    inputs = Map.merge(prompt_inputs.inputs, explicit_inputs)
    pending_ids = Enum.uniq(prompt_inputs.pending ++ explicit_pending)

    node =
      Node.new(
        id: id,
        ast_origin: agent.id,
        kind: :agent,
        envelope: agent.envelope,
        prompt_ref: prompt_ref,
        inputs: inputs,
        expansion_key: expansion_key,
        state: :pending
      )

    acc
    |> add_node(node)
    |> add_pending_if(agent.id, pending_ids)
    |> bind_node(bind_name, id)
  end

  defp expand_effect(%{kind: :exec} = exec, bind_name, acc) do
    id = node_id(exec.id, nil)

    {script_inputs, script_pending} = resolve_named_inputs(%{"script" => exec.script}, exec.id, acc)
    {extra_inputs, extra_pending} = resolve_inputs(exec.inputs, exec.id, acc)
    {timeout_inputs, timeout_pending} = resolve_timeout(exec.timeout, exec.id, acc)

    inputs = script_inputs |> Map.merge(extra_inputs) |> Map.merge(timeout_inputs)
    pending_ids = Enum.uniq(script_pending ++ extra_pending ++ timeout_pending)

    node =
      Node.new(
        id: id,
        ast_origin: exec.id,
        kind: :exec,
        inputs: inputs,
        expansion_key: nil,
        state: :pending
      )

    acc
    |> add_node(node)
    |> add_pending_if(exec.id, pending_ids)
    |> bind_node(bind_name, id)
  end

  defp expand_effect(%{kind: :subrun} = subrun, bind_name, acc) do
    id = node_id(subrun.id, nil)

    {source_inputs, source_pending} = resolve_named_inputs(%{"source" => subrun.source}, subrun.id, acc)
    {extra_inputs, extra_pending} = resolve_inputs(subrun.inputs, subrun.id, acc)

    inputs = Map.merge(source_inputs, extra_inputs)
    pending_ids = Enum.uniq(source_pending ++ extra_pending)

    node =
      Node.new(
        id: id,
        ast_origin: subrun.id,
        kind: :subrun,
        inputs: inputs,
        expansion_key: nil,
        state: :pending
      )

    acc
    |> add_node(node)
    |> add_pending_if(subrun.id, pending_ids)
    |> bind_node(bind_name, id)
  end

  # `when cond { body }`: a gate that emits its body only if `cond` is
  # truthy. When `cond` is not yet resolvable it materializes a `:gate`
  # placeholder whose input edge points at the node it waits on; when the
  # output arrives the re-expansion evaluates `cond` and emits the body
  # (recording the decision in the log).
  defp expand_effect(%{kind: :when} = node, bind_name, acc) do
    case resolve_value(node.cond, acc.env, acc.known) do
      {:value, cond_value} ->
        if truthy?(cond_value) do
          acc
          |> log_expansion(node.id, %{gate: :when, opened: true}, child_ids(node.body, node.id))
          |> expand_gate_body(node.body, node.id, bind_name)
        else
          log_expansion(acc, node.id, %{gate: :when, opened: false}, [])
        end

      {:node, dep_id, path} ->
        emit_gate(acc, node, dep_id, path, bind_name)

      :deferred ->
        emit_gate(acc, node, nil, [], bind_name)
    end
  end

  # `every n of counter { body }`: deterministic gate keyed on a persisted
  # counter recovered from the expansion log. It fires when the count of
  # prior firings makes the next tick a multiple of n. No wall clock.
  #
  # A construct is one tick per run. `expand_dynamic/1` re-runs `expand`
  # several times within a single run (at init, then after each node
  # success), feeding the grown log back in. If `every_nth` advanced the
  # tick on every pass it would drift forward inside one run and a cold
  # replay would never reproduce the live graph. So when the prior log
  # already holds this construct's tick, reproduce that recorded decision
  # idempotently; only a genuinely new run (no prior event for this origin)
  # computes a fresh tick from the counters recovered across prior runs.
  defp expand_effect(%{kind: :every_nth} = node, bind_name, acc) do
    case Map.get(acc.prior_ticks, counter_key(node.id, node.counter)) do
      # Already fired in the prior log: re-emit the body so the materializer
      # re-derives and idempotently merges the child, but do not re-log the
      # tick (the materializer adopts the prior log as is).
      %{fired: true} -> expand_gate_body(acc, node.body, node.id, bind_name)
      %{fired: false} -> acc
      nil -> evaluate_every_nth(node, bind_name, acc)
    end
  end

  # `map over as elem { body }`: fan out the body once per element of
  # `over`. When `over` is not resolvable it materializes a `:map_fanout`
  # placeholder; when the list resolves the re-expansion emits one child
  # per element with a stable expansion key.
  defp expand_effect(%{kind: :map} = node, bind_name, acc) do
    case resolve_value(node.over, acc.env, acc.known) do
      {:value, list} when is_list(list) ->
        emit_fanout(acc, node, list, bind_name)

      {:value, other} ->
        # A non-list `over` is a typed mismatch surfaced as an empty
        # fan-out rather than a crash; the runtime sees zero children.
        log_expansion(acc, node.id, %{gate: :map, over: :not_a_list, value: other}, [])

      {:node, dep_id, path} ->
        emit_map_placeholder(acc, node, dep_id, path, bind_name)

      :deferred ->
        emit_map_placeholder(acc, node, nil, [], bind_name)
    end
  end

  # Compute a fresh `every_nth` tick from the counters recovered across
  # prior runs' logs. Reached only on a run with no prior event for this
  # construct; a re-pass within a run reproduces the recorded decision
  # instead (see the `:every_nth` clause above).
  defp evaluate_every_nth(node, bind_name, acc) do
    fired = Map.get(acc.counters, counter_key(node.id, node.counter), 0)
    tick = fired + 1

    if rem(tick, node.n) == 0 do
      acc
      |> log_expansion(node.id, %{gate: :every_nth, counter: node.counter, tick: tick, fired: true}, child_ids(node.body, node.id))
      |> expand_gate_body(node.body, node.id, bind_name)
    else
      log_expansion(acc, node.id, %{gate: :every_nth, counter: node.counter, tick: tick, fired: false}, [])
    end
  end

  # --- dynamic placeholders ----------------------------------------------

  defp emit_gate(acc, node, dep_id, path, bind_name) do
    id = node_id(node.id, nil)
    inputs = gate_inputs(dep_id, path)

    placeholder =
      Node.new(
        id: id,
        ast_origin: node.id,
        kind: :gate,
        inputs: inputs,
        expansion_key: nil,
        state: :pending
      )

    acc
    |> add_node(placeholder)
    |> add_pending_if(node.id, deps_of(inputs))
    |> bind_node(bind_name, id)
  end

  defp emit_map_placeholder(acc, node, dep_id, path, bind_name) do
    id = node_id(node.id, nil)
    inputs = gate_inputs(dep_id, path)

    placeholder =
      Node.new(
        id: id,
        ast_origin: node.id,
        kind: :map_fanout,
        inputs: inputs,
        expansion_key: nil,
        state: :pending
      )

    acc
    |> add_node(placeholder)
    |> add_pending_if(node.id, deps_of(inputs))
    |> bind_node(bind_name, id)
  end

  # A map result binds to no single node (it is a bag of children), so a
  # `name <- map ...` binding is dropped: nothing downstream can read one
  # fan-out output as a scalar. The children themselves carry the edges.
  defp emit_fanout(acc, node, list, _bind_name) do
    emitted_ids =
      list
      |> Enum.with_index()
      |> Enum.map(fn {_elem, index} -> node_id(child_origin(node.body, node.id), {:fanout, node.id, index}) end)

    acc =
      list
      |> Enum.with_index()
      |> Enum.reduce(acc, fn {elem, index}, inner ->
        child_env = Map.put(inner.env, node.as, {:value, elem})
        key = {:fanout, node.id, index}

        inner
        |> with_env(child_env)
        |> expand_keyed_body(node.body, key)
        |> restore_env(inner.env)
      end)

    log_expansion(acc, node.id, %{gate: :map, count: length(list)}, emitted_ids)
  end

  # Expand a resolved `when`/`every_nth` body. When the gate itself is
  # bound (`result <- when ${x} { ... }`), `result` must point at the
  # body's emitted node, not at the placeholder (which is gone once the
  # gate resolves). Before this, a bound gate dropped its binding and any
  # downstream `${result...}` read silently lost its edge.
  defp expand_gate_body(acc, {:bind, inner, effect}, _gate_id, bind_name) do
    acc = expand_effect(effect, inner, acc)

    # `result <- when ... { n <- effect }`: alias the gate name to the same
    # body node the inner binding points at, so `result` and `n` both read
    # the body output. An unbound gate passes nil and this is a no-op.
    case {bind_name, Map.get(acc.env, inner)} do
      {nil, _} -> acc
      {_gate, nil} -> acc
      {gate, source} -> put_env(acc, gate, source)
    end
  end

  defp expand_gate_body(acc, {:let, _name, _pure} = body, _gate_id, _bind_name) do
    expand_statement(body, acc)
  end

  # A bare effect body (`result <- when ${x} { effect }`) binds the gate
  # name straight to the effect node.
  defp expand_gate_body(acc, effect, _gate_id, bind_name) when is_map(effect) do
    expand_effect(effect, bind_name, acc)
  end

  defp expand_keyed_body(acc, body, key) do
    expand_with_key(strip_binding(body), key, acc)
  end

  # --- input resolution ---------------------------------------------------

  # Resolve an inputs map (name => pure) into IR input refs plus the list
  # of node ids any deferred input still waits on.
  defp resolve_inputs(inputs, _origin, _acc) when map_size(inputs) == 0, do: {%{}, []}

  defp resolve_inputs(inputs, origin, acc) do
    resolve_named_inputs(inputs, origin, acc)
  end

  defp resolve_named_inputs(inputs, _origin, acc) do
    Enum.reduce(inputs, {%{}, []}, fn {name, pure}, {refs, pending} ->
      case resolve_value(pure, acc.env, acc.known) do
        {:value, value} -> {Map.put(refs, name, {:literal, value}), pending}
        {:node, id, path} -> {Map.put(refs, name, {:node, id, path}), pending}
        :deferred -> {refs, deferred_node_ids(pure, acc.env) ++ pending}
      end
    end)
  end

  defp resolve_timeout(nil, _origin, _acc), do: {%{}, []}

  defp resolve_timeout(pure, origin, acc) do
    resolve_named_inputs(%{"timeout" => pure}, origin, acc)
  end

  # The prompt's skill bindings (or inline interpolation) become inputs so
  # their node reads form dependency edges. The prompt_ref keeps the same
  # shape the IR layer expects: {:skill, name, bindings} | {:inline, text}.
  defp resolve_prompt({:skill, name, bindings}, origin, acc) do
    {refs, pending} = resolve_named_inputs(bindings, origin, acc)
    {{:skill, name, bindings_literal(bindings, acc)}, %{inputs: refs, pending: pending}}
  end

  defp resolve_prompt({:inline, pure}, _origin, acc) do
    case resolve_value(pure, acc.env, acc.known) do
      {:value, text} ->
        {{:inline, to_text(text)}, %{inputs: %{}, pending: []}}

      {:node, id, path} ->
        {{:inline, nil}, %{inputs: %{"prompt" => {:node, id, path}}, pending: []}}

      :deferred ->
        {{:inline, nil}, %{inputs: %{}, pending: deferred_node_ids(pure, acc.env)}}
    end
  end

  # Best-effort literal snapshot of skill bindings for the prompt_ref. Node
  # reads stay as their AST form; the resolved inputs carry the edges.
  defp bindings_literal(bindings, acc) do
    Map.new(bindings, fn {k, pure} ->
      case resolve_value(pure, acc.env, acc.known) do
        {:value, v} -> {k, v}
        _ -> {k, pure}
      end
    end)
  end

  # --- pure evaluation ----------------------------------------------------

  @typedoc "A resolved pure value, an edge to a node output, or unresolvable yet."
  @type resolution :: {:value, term()} | {:node, String.t(), [String.t()]} | :deferred

  @spec resolve_value(AST.pure(), map(), known_outputs()) :: resolution()
  defp resolve_value({:literal, value}, _env, _known), do: {:value, value}

  defp resolve_value({:var, name}, env, known) do
    case Map.get(env, name) do
      {:value, value} -> {:value, value}
      {:node_field, id, path} -> resolve_node(id, path, known)
      {:node, id} -> resolve_node(id, [], known)
      {:deferred, pure} -> resolve_value(pure, env, known)
      nil -> :deferred
    end
  end

  defp resolve_value({:field, base, path}, env, known) do
    case resolve_value(base, env, known) do
      {:value, value} -> {:value, dig(value, path)}
      {:node, id, base_path} -> {:node, id, base_path ++ path}
      :deferred -> :deferred
    end
  end

  defp resolve_value({:concat, parts}, env, known) do
    resolve_aggregate(parts, env, known, fn values -> Enum.map_join(values, "", &to_text/1) end)
  end

  defp resolve_value({:list, items}, env, known) do
    resolve_aggregate(items, env, known, & &1)
  end

  # An aggregate folds to a single value only when every part is a value.
  # If any part is a node read or deferred, the whole aggregate cannot be
  # one input_ref, so it is deferred until those node outputs resolve.
  defp resolve_aggregate(parts, env, known, combine) do
    resolved = Enum.map(parts, &resolve_value(&1, env, known))

    cond do
      Enum.all?(resolved, &match?({:value, _}, &1)) ->
        {:value, combine.(Enum.map(resolved, fn {:value, v} -> v end))}

      Enum.any?(resolved, &(&1 == :deferred)) ->
        :deferred

      true ->
        # Mix of literals and node reads. Defer until the node reads
        # resolve into known_outputs, then the aggregate folds.
        :deferred
    end
  end

  defp resolve_node(id, path, known) do
    case Map.fetch(known, id) do
      {:ok, output} -> {:value, dig(output, path)}
      :error -> {:node, id, path}
    end
  end

  # Read a (possibly nested) field from a value. Missing keys yield nil so
  # a typed mismatch surfaces downstream rather than crashing the expand.
  defp dig(value, []), do: value

  defp dig(value, [key | rest]) when is_map(value) do
    dig(Map.get(value, key) || Map.get(value, to_atom(key)), rest)
  end

  defp dig(_value, _path), do: nil

  defp to_atom(key) when is_binary(key) do
    String.to_existing_atom(key)
  rescue
    ArgumentError -> :"#{key}"
  end

  # --- helpers ------------------------------------------------------------

  defp gate_inputs(nil, _path), do: %{}
  defp gate_inputs(dep_id, path), do: %{"gate" => {:node, dep_id, path}}

  defp deps_of(inputs), do: Node.deps_from_inputs(inputs)

  defp truthy?(false), do: false
  defp truthy?(nil), do: false
  defp truthy?(_), do: true

  defp to_text(value) when is_binary(value), do: value
  defp to_text(value), do: to_string(value)

  defp deferred_node_ids(pure, env) do
    pure
    |> referenced_bindings()
    |> Enum.flat_map(fn name ->
      case Map.get(env, name) do
        {:node_field, id, _path} -> [id]
        {:node, id} -> [id]
        {:deferred, inner} -> deferred_node_ids(inner, env)
        _ -> []
      end
    end)
    |> Enum.uniq()
  end

  defp referenced_bindings({:var, name}), do: [name]
  defp referenced_bindings({:field, base, _path}), do: referenced_bindings(base)
  defp referenced_bindings({:concat, parts}), do: Enum.flat_map(parts, &referenced_bindings/1)
  defp referenced_bindings({:list, items}), do: Enum.flat_map(items, &referenced_bindings/1)
  defp referenced_bindings({:literal, _}), do: []

  # --- accumulator plumbing ----------------------------------------------

  defp add_node(acc, node), do: %{acc | nodes: [node | acc.nodes]}

  defp add_pending_if(acc, _origin, []), do: acc

  defp add_pending_if(acc, origin, needed) do
    %{acc | pending: [{:awaiting, origin, Enum.uniq(needed)} | acc.pending]}
  end

  defp put_env(acc, name, source), do: %{acc | env: Map.put(acc.env, name, source)}

  defp bind_node(acc, nil, _id), do: acc
  defp bind_node(acc, name, id), do: put_env(acc, name, {:node, id})

  defp with_env(acc, env), do: %{acc | env: env}
  defp restore_env(acc, env), do: %{acc | env: env}

  defp log_expansion(acc, origin, observed, emitted) do
    event = %{origin: origin, observed: observed, emitted: emitted}
    %{acc | log: [event | acc.log]}
  end

  # Stable, content-derived id: the ast origin plus the expansion key, so a
  # deterministic replay rebuilds the identical id.
  defp node_id(ast_origin, nil), do: ast_origin

  defp node_id(ast_origin, key) do
    digest = :crypto.hash(:sha256, :erlang.term_to_binary({ast_origin, key}))
    ast_origin <> "-" <> (digest |> Base.encode16(case: :lower) |> binary_part(0, 8))
  end

  # A combinator body keeps its own ast origin so the child id is distinct
  # from a top-level node of the same shape.
  defp child_origin(%{id: id}, _gate_id), do: id
  defp child_origin({:bind, _name, %{id: id}}, _gate_id), do: id
  defp child_origin(_body, gate_id), do: gate_id

  defp child_ids(body, gate_id) do
    [node_id(child_origin(body, gate_id), nil)]
  end

  defp strip_binding({:bind, _name, expr}), do: expr
  defp strip_binding(expr), do: expr

  # Re-expand a single statement under a fan-out expansion key so each
  # emitted child gets a distinct, stable id.
  defp expand_with_key(expr, key, acc) when is_map(expr) do
    expand_effect_keyed(expr, key, acc)
  end

  defp expand_effect_keyed(%{kind: kind} = expr, key, acc)
       when kind in [:agent, :exec, :subrun] do
    id = node_id(expr.id, key)

    {inputs, pending_ids, prompt_ref, envelope} = keyed_node_parts(expr, acc)

    node =
      Node.new(
        id: id,
        ast_origin: expr.id,
        kind: kind,
        envelope: envelope,
        prompt_ref: prompt_ref,
        inputs: inputs,
        expansion_key: key,
        state: :pending
      )

    acc
    |> add_node(node)
    |> add_pending_if(expr.id, pending_ids)
  end

  defp keyed_node_parts(%{kind: :agent} = agent, acc) do
    {prompt_ref, prompt_inputs} = resolve_prompt(agent.prompt, agent.id, acc)
    {explicit_inputs, explicit_pending} = resolve_inputs(agent.inputs, agent.id, acc)
    inputs = Map.merge(prompt_inputs.inputs, explicit_inputs)
    {inputs, Enum.uniq(prompt_inputs.pending ++ explicit_pending), prompt_ref, agent.envelope}
  end

  defp keyed_node_parts(%{kind: :exec} = exec, acc) do
    {script_inputs, script_pending} = resolve_named_inputs(%{"script" => exec.script}, exec.id, acc)
    {extra_inputs, extra_pending} = resolve_inputs(exec.inputs, exec.id, acc)
    {timeout_inputs, timeout_pending} = resolve_timeout(exec.timeout, exec.id, acc)
    inputs = script_inputs |> Map.merge(extra_inputs) |> Map.merge(timeout_inputs)
    {inputs, Enum.uniq(script_pending ++ extra_pending ++ timeout_pending), nil, nil}
  end

  defp keyed_node_parts(%{kind: :subrun} = subrun, acc) do
    {source_inputs, source_pending} = resolve_named_inputs(%{"source" => subrun.source}, subrun.id, acc)
    {extra_inputs, extra_pending} = resolve_inputs(subrun.inputs, subrun.id, acc)
    inputs = Map.merge(source_inputs, extra_inputs)
    {inputs, Enum.uniq(source_pending ++ extra_pending), nil, nil}
  end

  # --- expansion-log counters --------------------------------------------

  # Recover each every_nth counter's tick total from the prior log so a
  # replay reconstructs the identical gate decisions without a live tick.
  # Every recorded evaluation is a tick, fired or skipped: the gate fires
  # on the nth tick, so the count must include the skips between fires.
  defp counters_from_log(log) do
    Enum.reduce(log, %{}, fn event, acc ->
      case event do
        %{origin: origin, observed: %{gate: :every_nth, counter: counter}} ->
          Map.update(acc, counter_key(origin, counter), 1, &(&1 + 1))

        _ ->
          acc
      end
    end)
  end

  # The most recent recorded tick decision per `every_nth` construct in the
  # prior log. A re-expansion within the same run reproduces this decision
  # rather than advancing the tick, so the gate is idempotent across the
  # several `expand_dynamic/1` passes one run makes. A new run carries no
  # event for the origin and computes a fresh tick from `counters_from_log`.
  defp prior_ticks_from_log(log) do
    Enum.reduce(log, %{}, fn event, acc ->
      case event do
        %{origin: origin, observed: %{gate: :every_nth, counter: counter, fired: fired}} ->
          Map.put(acc, counter_key(origin, counter), %{fired: fired})

        _ ->
          acc
      end
    end)
  end

  defp counter_key(origin, counter), do: {origin, counter}
end
