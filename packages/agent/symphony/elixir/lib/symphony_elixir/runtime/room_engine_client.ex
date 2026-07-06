defmodule SymphonyElixir.Runtime.RoomEngineClient do
  @moduledoc """
  The production `Runtime.EngineClient`: it runs a node's attempt by
  lowering the node to an engine turn and submitting it through
  `Engine.Client` to the room-server. This is the WS-4 implementation of
  the behaviour the IR runtime depends on; tests still inject an
  in-process fake, so the runtime never requires a live room-server.

  The runtime hands this module an `IR.Node` and per-attempt context
  (`run_id`, `attempt`, and the resolved working directory and
  room-server URL). The module turns the node's `envelope` and
  `prompt_ref` into the `Engine.Client` turn shape, submits it, and maps
  the room-server outcome back to the behaviour's `result()` triple
  (carrying the engine's `thread_id` even on failure so the runtime can
  record it for a later reattach probe).

  ## Prompt resolution

  A node's `prompt_ref` is either `{:inline, text}` or
  `{:skill, ref, bindings}`. Both are rendered through
  `SymphonyElixir.Prompt`: inline text passes through, and a skill ref is
  rendered from the active pack's skill body interpolated with the
  bindings the interpreter resolved. The skill-body resolver is injectable
  through `run_opts[:skill_resolver]` for tests; production defaults to the
  `Catalog`, which already expands shared `{{partial:_}}` includes when it
  loads a skill. A skill that names an input the node never bound is a
  render error, so a half-rendered prompt never reaches an engine.

  ## status/1 and restart reattach

  The synchronous `/api/agent/turns` path the room-server exposes today
  runs the whole turn in one request and has no probe-by-thread route, so
  `status/1` cannot ask the engine whether an orphaned thread is still
  alive. It returns `:unknown`, the conservative answer: recovery strands
  the node (or auto-retries only under the opt-in side-effect-free
  policy). A real reattach probe needs a room-server status route and
  lands with the streaming client.
  """

  @behaviour SymphonyElixir.Runtime.EngineClient

  alias SymphonyElixir.Catalog
  alias SymphonyElixir.Engine.Client
  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.Prompt

  @impl true
  def run_node(%Node{kind: :agent, envelope: %Envelope{} = envelope} = node, run_opts) do
    with {:ok, body} <- resolve_prompt(node.prompt_ref, run_opts),
         {:ok, cwd} <- fetch_cwd(run_opts) do
      prompt = append_input_block(body, Map.get(run_opts, :trigger))

      turn = %{
        prompt: prompt,
        cwd: cwd,
        tools: Map.get(run_opts, :tools, []),
        run_id: Map.get(run_opts, :run_id),
        node_id: node.id
      }

      envelope
      |> Client.submit_turn(turn, client_opts(run_opts))
      |> to_result()
    else
      {:error, reason} -> {:error, reason, nil}
    end
  end

  def run_node(%Node{kind: :agent} = node, _run_opts) do
    {:error, {:missing_envelope, node.id}, nil}
  end

  def run_node(%Node{kind: kind} = node, _run_opts) do
    # Only :agent nodes go through the engine host. :exec/:subrun/:gate
    # nodes have their own executors; routing one here is a wiring bug, so
    # fail loudly rather than submit a meaningless engine turn.
    {:error, {:not_an_agent_node, kind, node.id}, nil}
  end

  @impl true
  def status(_thread_id) do
    # No probe-by-thread route on the synchronous path; see moduledoc.
    :unknown
  end

  # Render the prompt through `SymphonyElixir.Prompt`. Inline text passes
  # through; a skill ref is rendered from the active pack's skill body and
  # the bindings the interpreter resolved. The skill-body resolver is
  # injectable through `run_opts[:skill_resolver]` (tests pass a fake);
  # production defaults to the `Catalog`, which already expands shared
  # `{{partial:_}}` includes when it loads a skill, so no partial resolver
  # is needed here.
  defp resolve_prompt(prompt_ref, run_opts) do
    Prompt.build(prompt_ref, resolver: skill_resolver(run_opts))
  end

  # Append the run's trigger context to the agent prompt as an `<input>`
  # block. Every dispatch-driven skill body documents reading its payload
  # (the cron envelope with `scheduled_for`/`fired_at`/`input`, a webhook
  # event, or a manual input map) from this block; `Ingress` stamps the
  # trigger onto `graph.trigger` and the IR runtime forwards it here as
  # `run_opts[:trigger]`, so this is the one place it reaches the engine
  # prompt. An operator-started run carries no trigger and appends nothing,
  # leaving the skill body verbatim.
  defp append_input_block(prompt, nil), do: prompt

  defp append_input_block(prompt, trigger) do
    prompt <> "\n\n<input>\n" <> Jason.encode!(trigger, pretty: true) <> "\n</input>\n"
  end

  defp skill_resolver(run_opts) do
    case Map.get(run_opts, :skill_resolver) do
      fun when is_function(fun, 1) -> fun
      _ -> &catalog_skill_body/1
    end
  end

  defp catalog_skill_body(name) do
    case Catalog.skill(name) do
      {:ok, skill} -> {:ok, skill.body}
      {:error, :not_found} -> {:error, {:skill_not_found, name}}
    end
  end

  defp fetch_cwd(run_opts) do
    case Map.get(run_opts, :cwd) do
      cwd when is_binary(cwd) and cwd != "" -> {:ok, cwd}
      _ -> {:error, :missing_cwd}
    end
  end

  # Pass the room-server URL and any Req injection from the runtime's
  # per-attempt context straight through to the client. `run_id` and an
  # optional `placement` module ride along so the client can resolve an
  # `:ixvm` envelope to the run's own provisioned room-server. Drop nils
  # so the client's own defaults apply.
  defp client_opts(run_opts) do
    Enum.reject(
      [
        room_server_url: Map.get(run_opts, :room_server_url),
        req_options: Map.get(run_opts, :req_options, []),
        timeout_ms: Map.get(run_opts, :timeout_ms),
        run_id: Map.get(run_opts, :run_id),
        placement: Map.get(run_opts, :placement)
      ],
      fn {_k, v} -> is_nil(v) end
    )
  end

  defp to_result({:ok, %{thread_id: thread_id} = output}), do: {:ok, output, thread_id}
  defp to_result({:error, {:turn_error, _msg, thread_id} = reason}), do: {:error, reason, thread_id}
  defp to_result({:error, {:turn_cancelled, thread_id} = reason}), do: {:error, reason, thread_id}
  defp to_result({:error, reason}), do: {:error, reason, nil}
end
