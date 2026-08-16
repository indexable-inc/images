defmodule IxMcp.EventLog do
  @moduledoc """
  The append-only record of what a recursive analysis did, and the
  derivation cache that makes it cheap to repeat.

  One table (`rlm_events`, `IxMcp.ActionLog` rung 11), two jobs:

    * AUDIT. Every sub-model call, cache hit and budget refusal is a row,
      with model, prompt hash, context ids, token counts and latency. A run
      that spends real money can be accounted for after the fact.
    * DERIVATION. `lm_result` rows are keyed by `cache_key` -- blake3 over
      (model, canonical prompt, sorted context ids) -- so `IxMcp.LM.ask/2`
      answers a repeated question from the log. Same question over the same
      bytes is free; a log that GREW pays only for the chunks that are new.

  Keeping both in one table is deliberate. A separate cache could disagree
  with the audit trail, and then the trail no longer answers what was
  spent.

  Payloads over 4 KB spill into `IxMcp.EventLog.Cas` and the row
  keeps only the blake3 id, for the same reason `IxMcp.Ctx` hands out
  handles: a log is read far more often than a payload is needed.

  Storage is swappable behind `append/1` and `fold/3`: callers never see
  SQL, JSON or the CAS.
  """

  alias IxMcp.ActionLog
  alias IxMcp.EventLog.Cas
  alias IxMcp.Workspace

  @inline_limit 4_096

  @typedoc """
  What happened. `:lm_ask` and `:lm_result` pair up per call; `:stdlib_call`
  is one grown-stdlib resident call (`IxMcp.Stdlib.observe/3`).
  """
  @type kind ::
          :lm_ask
          | :lm_result
          | :lm_cache_hit
          | :lm_error
          | :lm_budget_refused
          | :stdlib_call

  @typedoc """
  A stored event. `payload` is the decoded payload; `text` is present when
  the event carried one (inline, or fetched back from the CAS on demand).
  """
  @type event :: %{
          seq: non_neg_integer(),
          ts: String.t(),
          kind: kind(),
          payload: map(),
          content_id: String.t() | nil,
          cache_key: String.t() | nil,
          workspace: String.t() | nil
        }

  @doc """
  Append an event. Returns its `seq`, or 0 when the log is degraded.

  `event` needs a `:kind` and may carry `:payload` (a JSON-encodable map),
  `:text` (spilled to the CAS when large), and `:cache_key`. The workspace
  is stamped from `IxMcp.Workspace.current/0` unless given.
  """
  @spec append(map()) :: non_neg_integer()
  def append(%{kind: kind} = event) do
    {payload, content_id} = encode_payload(event)

    ActionLog.append_rlm_event(
      %{
        kind: kind,
        payload_json: payload,
        content_id: content_id,
        cache_key: Map.get(event, :cache_key),
        workspace: Map.get(event, :workspace) || to_string(Workspace.current())
      },
      server()
    )
  end

  @doc """
  Fold the log forward, oldest first.

  Options: `:after` (exclusive seq cursor), `:kind`, `:batch` (rows per
  read, default 500). Reads in batches so a long log folds without loading
  itself into memory, and the cursor means a grown log resumes rather than
  restarts.

      EventLog.fold(0, fn event, acc -> acc + tokens(event) end)
  """
  @spec fold(term(), (event(), term() -> term()), keyword()) :: term()
  def fold(acc, fun, opts \\ []) when is_function(fun, 2) do
    batch = Keyword.get(opts, :batch, 500)
    kind = Keyword.get(opts, :kind)
    do_fold(Keyword.get(opts, :after, 0), acc, fun, batch, kind)
  end

  @doc """
  Events as a list, oldest first. Options as `fold/3` plus `:limit`.
  """
  @spec events(keyword()) :: [event()]
  def events(opts \\ []) do
    limit = Keyword.get(opts, :limit, 200)

    [after: Keyword.get(opts, :after, 0), limit: limit, kind: Keyword.get(opts, :kind)]
    |> ActionLog.rlm_events(server())
    |> Enum.map(&decode/1)
  end

  @doc """
  The cached result for `cache_key`, with its text materialized, or nil.

  nil covers every uncertain case -- no row, or a spilled payload whose
  blob is gone -- because a cache that guesses is worse than a cache miss.
  """
  @spec cached(String.t()) :: event() | nil
  def cached(cache_key) when is_binary(cache_key) do
    case ActionLog.rlm_cached(cache_key, server()) do
      nil -> nil
      row -> materialize(decode(row))
    end
  end

  @doc """
  The payload text of `event`, fetching it from the CAS if it spilled.
  """
  @spec text(event()) :: {:ok, String.t()} | {:error, :missing}
  def text(%{content_id: nil} = event), do: {:ok, Map.get(event.payload, "text", "")}
  def text(%{content_id: id}), do: Cas.get(id)

  @doc "The inline payload ceiling, in bytes; larger payloads go to the CAS."
  @spec inline_limit() :: pos_integer()
  def inline_limit, do: @inline_limit

  @doc """
  Which `IxMcp.ActionLog` instance stores the log.

  A seam, not a feature: the kernel has exactly one action log, and a test
  that wrote RLM events into it would be writing into the developer's real
  ledger. `config :ix_mcp, :action_log_server` points this elsewhere.
  """
  @spec server() :: GenServer.server()
  def server, do: Application.get_env(:ix_mcp, :action_log_server, ActionLog)

  # ── internals ─────────────────────────────────────────────────────────

  @spec do_fold(non_neg_integer(), term(), (event(), term() -> term()), pos_integer(), term()) ::
          term()
  defp do_fold(cursor, acc, fun, batch, kind) do
    case ActionLog.rlm_events([after: cursor, limit: batch, kind: kind], server()) do
      [] ->
        acc

      rows ->
        acc = Enum.reduce(rows, acc, fn row, inner -> fun.(decode(row), inner) end)
        last = List.last(rows).seq
        if length(rows) < batch, do: acc, else: do_fold(last, acc, fun, batch, kind)
    end
  end

  # Text is what spills; the rest of the payload stays queryable in the row.
  @spec encode_payload(map()) :: {String.t(), String.t() | nil}
  defp encode_payload(event) do
    payload = Map.get(event, :payload, %{})

    case Map.get(event, :text) do
      nil ->
        {JSON.encode!(payload), nil}

      text when byte_size(text) <= @inline_limit ->
        {JSON.encode!(Map.put(payload, :text, text)), nil}

      text ->
        case Cas.put(text) do
          {:ok, id} ->
            {JSON.encode!(Map.merge(payload, %{spilled: true, bytes: byte_size(text)})), id}

          {:error, _reason} ->
            {JSON.encode!(Map.put(payload, :text, text)), nil}
        end
    end
  end

  @spec decode(map()) :: event()
  defp decode(row) do
    row
    |> Map.delete(:payload_json)
    |> Map.put(:payload, JSON.decode!(row.payload_json))
  end

  @spec materialize(event()) :: event() | nil
  defp materialize(%{content_id: nil} = event), do: event

  defp materialize(%{content_id: id} = event) do
    case Cas.get(id) do
      {:ok, text} -> put_in(event.payload["text"], text)
      {:error, _reason} -> nil
    end
  end
end
