defmodule SymphonyElixir.Engine.Client do
  @moduledoc """
  The single door between the Elixir runtime and the room-server engine
  host. Every engine turn the runtime issues passes through here, and
  nothing else in `elixir/lib/` speaks the room-server wire. This is the
  layer boundary the overhaul encodes: DSL -> IR -> Runtime ->
  `Engine.Client` -> room-server, with `Engine.Client` the only module
  that names the room-server's HTTP contract.

  ## What it owns

  - Lowering a typed `Engine.Envelope` plus a prompt and working
    directory into the room-server's canonical `TurnRequest` JSON
    (camelCase keys, the engine-agnostic shape from room-server's
    `engine.rs` in the IX monorepo, mirrored by `contracts/fixtures`).
  - Resolving the envelope's `location` to a concrete room-server base
    URL. `:local`, `{:room, url}`, and the default config URL are
    handled here; `:ixvm` and `{:host, name}` both resolve to the per-run
    URL the run's `Runtime.Placement` provisioned (the run acquires its
    own room-server before the first agent turn), looked up by `run_id`.
    The same `run_id` lookup also serves an `:ixvm` node whose VM setup
    failed and fell back to a host room-server, since the fallback
    registers under the same `run_id`. A turn submitted without the
    runtime's `run_id` context fails with a clear `{:unresolved_location,
    _}` rather than silently routing to the default server.
  - Speaking `POST /api/agent/turns` and parsing `AgentTurnResponse`
    (`threadId`, `outcome`, `eventCount`, `usage`) into a runtime-facing
    result, mapping the turn's terminal `usage` totals to the
    `IR.Attempt.cost` shape so per-turn cost reaches the run.

  The room-server runs the whole turn and returns its terminal outcome
  plus the thread id it assigned, so the synchronous `submit_turn/2` is
  the natural shape for a runtime that schedules one node attempt as one
  monitored task. A streaming surface (subscribe to `EngineEvent`s,
  answer approvals, interrupt) is a later addition behind the same module;
  the synchronous turn is the smallest contract that drives a node to a
  terminal state.

  ## Known limitations

  The room-server's `/api/agent/turns` is request/response: it blocks
  until the turn completes. A turn that runs for an hour holds the HTTP
  connection open for that hour, so `submit_turn/2` sets a long receive
  timeout and the caller must run it off the runtime process (the runtime
  already schedules each attempt in a monitored task). Approvals and
  interrupts are not reachable through this synchronous path; an engine
  configured for `:read_only` or `:workspace_write` that pauses for an
  approval would stall until the timeout. Use `:danger_full_access` (or a
  self-executing engine) for the synchronous path until the streaming
  client lands.
  """

  alias SymphonyElixir.Engine.Envelope

  @default_timeout_ms to_timeout(hour: 1)

  @typedoc """
  Everything a turn needs beyond the envelope: the prompt text the engine
  runs, the working directory it runs in, the dynamic-tool specs the host
  will execute, and the runtime correlation ids echoed on every event.
  """
  @type turn :: %{
          required(:prompt) => String.t(),
          required(:cwd) => String.t(),
          optional(:tools) => [map()],
          optional(:run_id) => String.t() | nil,
          optional(:node_id) => String.t() | nil
        }

  @typedoc """
  Result of a completed turn. `thread_id` is the engine handle the turn
  opened, carried even on failure so the runtime can record it for a
  later reattach probe.
  """
  @type result ::
          {:ok,
           %{
             thread_id: String.t(),
             event_count: non_neg_integer(),
             cost: cost() | nil
           }}
          | {:error, term()}

  @typedoc """
  The turn's terminal token/cost totals, already mapped to the
  `IR.Attempt.cost` shape so the runtime can store it without a second
  translation. `nil` when the room-server reported no usage (an older
  server or a turn that emitted none), so the runtime records "unknown"
  rather than a sham zero. `:usd` is present only when the engine priced
  the turn.
  """
  @type cost :: %{
          optional(:usd) => float(),
          optional(:tokens_in) => non_neg_integer(),
          optional(:tokens_out) => non_neg_integer(),
          optional(:cache_read) => non_neg_integer(),
          optional(:cache_creation) => non_neg_integer()
        }

  @doc """
  Run one engine turn through the room-server and return its terminal
  outcome. Lowers `envelope` to a `TurnRequest`, resolves the target
  room-server URL, POSTs `/api/agent/turns`, and maps the response.

  `opts` carries the room-server URL resolution context:

  - `:room_server_url` - the default room-server base URL, used when the
    envelope location is `:local` or a default. Usually the value from
    `Config`.
  - `:req_options` - extra options merged into the `Req` request (tests
    inject a `:plug` or `:base_url` stub here).
  - `:timeout_ms` - receive timeout for the (long) turn. Defaults to one
    hour.
  """
  @spec submit_turn(Envelope.t(), turn(), keyword()) :: result()
  def submit_turn(%Envelope{} = envelope, turn, opts \\ []) when is_map(turn) and is_list(opts) do
    with {:ok, base_url} <- resolve_base_url(envelope.location, opts),
         {:ok, body} <- request_body(envelope, turn) do
      post_turn(base_url, body, opts)
    end
  end

  @doc """
  Lower a typed envelope plus a turn into the room-server's `TurnRequest`
  JSON map (camelCase keys). Public so a test can assert the wire shape
  without a running server, and so a caller can inspect what it would
  send.
  """
  @spec request_body(Envelope.t(), turn()) :: {:ok, map()} | {:error, term()}
  def request_body(%Envelope{} = envelope, turn) when is_map(turn) do
    with {:ok, prompt} <- fetch_prompt(turn),
         {:ok, cwd} <- fetch_cwd(turn) do
      body =
        %{
          "engine" => Atom.to_string(envelope.engine),
          "model" => envelope.model,
          "permissions" => Atom.to_string(envelope.permissions),
          "cwd" => cwd,
          "prompt" => prompt,
          "tools" => Map.get(turn, :tools, []),
          "runId" => Map.get(turn, :run_id),
          "nodeId" => Map.get(turn, :node_id)
        }
        |> put_effort(envelope.effort)
        |> drop_nil()

      {:ok, body}
    end
  end

  # The room-server omits `effort` when null (serde skip_serializing_if),
  # so the request only carries it when the envelope declared a budget.
  defp put_effort(body, nil), do: body
  defp put_effort(body, effort), do: Map.put(body, "effort", Atom.to_string(effort))

  defp drop_nil(body), do: Map.reject(body, fn {_k, v} -> is_nil(v) end)

  defp fetch_prompt(%{prompt: prompt}) when is_binary(prompt) and prompt != "", do: {:ok, prompt}
  defp fetch_prompt(_), do: {:error, :missing_prompt}

  defp fetch_cwd(%{cwd: cwd}) when is_binary(cwd) and cwd != "", do: {:ok, cwd}
  defp fetch_cwd(_), do: {:error, :missing_cwd}

  # Location resolution is the deployment-topology seam. The synchronous
  # client routes `:local` and an explicit `{:room, url}` to a fixed URL,
  # and `:ixvm` / `{:host, _}` to the per-run room-server the run's
  # `Runtime.Placement` provisioned, looked up by `run_id`. Failing loudly
  # here keeps a turn from silently running on the wrong server.
  defp resolve_base_url(:local, opts), do: fetch_default_url(opts)
  defp resolve_base_url({:room, url}, _opts) when is_binary(url) and url != "", do: {:ok, url}
  defp resolve_base_url(:ixvm, opts), do: fetch_placement_url(:ixvm, opts)
  defp resolve_base_url({:host, _} = location, opts), do: fetch_placement_url(location, opts)

  defp resolve_base_url(other, _opts), do: {:error, {:invalid_location, other}}

  # The run's `Runtime.Placement` provisioned its own room-server and
  # registered the URL under `run_id` before the first agent turn, so the
  # turn routes there rather than to the shared default. The placement
  # module is injectable through `opts[:placement]` so a test can resolve
  # against a stub without a real VM; production defaults to
  # `Runtime.Placement`. A missing `run_id` (a turn submitted without the
  # runtime context) or an unresolved run is an explicit error, never a
  # silent fall-through to the default server.
  defp fetch_placement_url(location, opts) do
    placement = Keyword.get(opts, :placement, SymphonyElixir.Runtime.Placement)

    case Keyword.get(opts, :run_id) do
      run_id when is_binary(run_id) and run_id != "" ->
        case placement.base_url(run_id) do
          {:ok, url} when is_binary(url) and url != "" -> {:ok, url}
          _ -> {:error, {:unresolved_location, location}}
        end

      _ ->
        {:error, {:unresolved_location, location}}
    end
  end

  defp fetch_default_url(opts) do
    case Keyword.get(opts, :room_server_url) do
      url when is_binary(url) and url != "" -> {:ok, url}
      _ -> {:error, :missing_room_server_url}
    end
  end

  defp post_turn(base_url, body, opts) do
    timeout = Keyword.get(opts, :timeout_ms, @default_timeout_ms)
    req_options = Keyword.get(opts, :req_options, [])

    request = Keyword.merge([url: join(base_url, "/api/agent/turns"), json: body, receive_timeout: timeout, connect_options: [timeout: 30_000]], req_options)

    case Req.post(request) do
      {:ok, %{status: status, body: response}} when status in 200..299 ->
        parse_response(response)

      {:ok, %{status: status, body: response}} ->
        {:error, {:agent_turn_status, status, response}}

      {:error, reason} ->
        {:error, {:agent_turn_failed, reason}}
    end
  end

  defp parse_response(%{"outcome" => outcome} = response) do
    thread_id = Map.get(response, "threadId", "")
    event_count = Map.get(response, "eventCount", 0)
    cost = parse_cost(Map.get(response, "usage"))

    case outcome do
      %{"kind" => "ok"} ->
        {:ok, %{thread_id: thread_id, event_count: event_count, cost: cost}}

      %{"kind" => "error", "message" => message} ->
        {:error, {:turn_error, message, thread_id}}

      %{"kind" => "cancelled"} ->
        {:error, {:turn_cancelled, thread_id}}

      other ->
        {:error, {:unexpected_outcome, other}}
    end
  end

  defp parse_response(other), do: {:error, {:unexpected_agent_response, other}}

  # Map the room-server's `Usage` (camelCase) to the `IR.Attempt.cost`
  # shape. A response without `usage` (older server) yields nil so the
  # attempt records "unknown" rather than a sham zero; `costUsd` is dropped
  # when the engine did not price the turn, so a present `:usd` always
  # means a real number.
  defp parse_cost(usage) when is_map(usage) do
    drop_nil(%{
      usd: Map.get(usage, "costUsd"),
      tokens_in: Map.get(usage, "tokensIn", 0),
      tokens_out: Map.get(usage, "tokensOut", 0),
      cache_read: Map.get(usage, "cacheRead", 0),
      cache_creation: Map.get(usage, "cacheCreation", 0)
    })
  end

  defp parse_cost(_), do: nil

  defp join(base_url, path) do
    String.trim_trailing(base_url, "/") <> path
  end
end
