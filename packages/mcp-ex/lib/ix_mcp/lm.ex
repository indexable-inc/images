defmodule IxMcp.LM do
  @moduledoc """
  Sub-model calls over context handles: the recursion in "recursive
  language model".

  A cell asks a cheap model a narrow question about a slice it never had to
  read itself:

      log = Ctx.file!("/var/log/huge.log")

      log
      |> Ctx.grep(~r/ERROR/)
      |> Enum.map(fn {_no, _line, slice} -> slice end)
      |> LM.map(fn _slice -> "One line: what failed and why?" end)

  Three things make this different from calling a model in a loop, and all
  three are the point (see `IxMcp.RLM`):

    * PARALLEL. `map/3` fans out over `Task.async_stream`. The paper's
      implementation blocks on each sub-call in turn; a fan-out of 40 chunks
      at concurrency 8 finishes in about an eighth of the time, and the
      recursion is embarrassingly parallel by construction.
    * MEMOIZED. Every call is keyed by blake3 over (model, canonical prompt,
      sorted context ids) and answered from `IxMcp.EventLog` on a repeat. A
      sub-call is a DERIVATION of its inputs, so re-analysing a log that grew
      pays only for the chunks that are new.
    * METERED. Nothing calls a provider without a reservation from
      `IxMcp.LM.Budget`, and an exhausted budget is an error, never a
      quietly smaller answer.

  ## Depth

  v0 sub-calls are plain completions: depth 1, no tools, no nested REPL. A
  sub-call cannot itself call `LM.ask/2`. `mode: :rlm` is reserved for the
  recursive case and REFUSES today rather than silently doing the shallow
  thing.

  ## Provider

  `IxMcp.LM.Anthropic` by default, over OTP's `:httpc` like `IxMcp.Web`
  (#3798), keyed by `ANTHROPIC_API_KEY` in the kernel's environment. Swap it
  with `config :ix_mcp, :lm_backend, IxMcp.LM.Stub` for a dry run.
  """

  alias IxMcp.Blake3
  alias IxMcp.Ctx
  alias IxMcp.EventLog
  alias IxMcp.LM.Budget
  alias IxMcp.Workspace

  @default_model "claude-haiku-4-5"
  @default_max_tokens 4_096
  @default_ctx_limit 20_000
  @default_concurrency 8
  @default_timeout_ms 120_000

  @typedoc "Why a call did not produce an answer."
  @type error ::
          :budget_exhausted
          | :rlm_mode_unimplemented
          | :timeout
          | {:http, non_neg_integer(), String.t()}
          | {:transport, term()}
          | term()

  @doc """
  Ask a sub-model `prompt`, optionally over context handles.

  Options:

    * `:ctx` — an `IxMcp.Ctx` handle or a list of them. Each is materialized
      into the sub-call's prompt (capped by `:ctx_limit`), which is the one
      place bytes are supposed to cross into a model's window.
    * `:ctx_limit` — bytes per handle, default 20_000.
    * `:model` — default `"claude-haiku-4-5"`, a cheap fast tier because
      sub-calls are many and narrow. Configurable with
      `config :ix_mcp, :lm_model`.
    * `:max_tokens` — default 4096. Kept modest on purpose: the transport is
      non-streaming, and large non-streaming completions are refused by the
      provider.
    * `:system` — system prompt.
    * `:cache` — default true. `false` skips the cache probe but still logs.
    * `:mode` — `:flat` (default). `:rlm` is reserved and returns
      `{:error, :rlm_mode_unimplemented}`.

  Returns `{:ok, text}` or `{:error, reason}`. Never raises for a budget or
  provider condition; a missing API key does raise, because an absent
  credential is a setup error rather than something to degrade around.
  """
  @spec ask(String.t(), keyword()) :: {:ok, String.t()} | {:error, error()}
  def ask(prompt, opts \\ []) when is_binary(prompt) do
    case Keyword.get(opts, :mode, :flat) do
      :flat -> do_ask(prompt, opts)
      :rlm -> {:error, :rlm_mode_unimplemented}
      other -> {:error, {:unknown_mode, other}}
    end
  end

  @doc """
  Ask about many items at once, in parallel.

  `prompt_fun` receives each item and returns a prompt, or `{prompt, opts}`
  to vary the options per item. An item that is an `IxMcp.Ctx` handle is
  passed as that call's `:ctx` automatically, which makes the common shape
  -- one question, every chunk -- a one-liner.

  Options: `:concurrency` (default 8), `:timeout` per call (default 120s),
  `:ordered` (default true), plus everything `ask/2` takes.

  Results line up with the inputs. A fan-out that exhausts the budget
  half-way returns `{:error, :budget_exhausted}` for the rest instead of
  quietly returning fewer answers.
  """
  @spec map([term()], (term() -> String.t() | {String.t(), keyword()}), keyword()) :: [
          {:ok, String.t()} | {:error, error()}
        ]
  def map(items, prompt_fun, opts \\ []) when is_list(items) and is_function(prompt_fun, 1) do
    {concurrency, opts} = Keyword.pop(opts, :concurrency, @default_concurrency)
    {timeout, opts} = Keyword.pop(opts, :timeout, @default_timeout_ms)
    {ordered, opts} = Keyword.pop(opts, :ordered, true)

    # The workspace is a process-dictionary fact (IxMcp.Workspace.current/0) and
    # a Task does NOT inherit the process dictionary, so a fan-out would meter
    # and stamp itself against the default workspace instead of the caller's --
    # a budget that leaks across workspaces exactly where spend is highest.
    workspace = Workspace.current()

    items
    |> Task.async_stream(
      fn item ->
        Process.put(:ix_workspace, workspace)
        ask_item(item, prompt_fun, opts)
      end,
      max_concurrency: concurrency,
      timeout: timeout,
      ordered: ordered,
      on_timeout: :kill_task
    )
    |> Enum.map(fn
      {:ok, result} -> result
      {:exit, :timeout} -> {:error, :timeout}
      {:exit, reason} -> {:error, {:exit, reason}}
    end)
  end

  @doc "This workspace's spend in the current window. See `IxMcp.LM.Budget`."
  @spec budget() :: map()
  def budget, do: Budget.state()

  @doc "The model used when a call does not name one."
  @spec default_model() :: String.t()
  def default_model, do: Application.get_env(:ix_mcp, :lm_model, @default_model)

  @doc "Provider module, swappable so tests exercise the cache and the budget against a deterministic stub instead of the network."
  @spec backend() :: module()
  def backend, do: Application.get_env(:ix_mcp, :lm_backend, IxMcp.LM.Anthropic)

  @doc """
  The cache key a call would use: blake3 over model, system, prompt and the
  sorted context ids. Exposed because a cache you cannot inspect is a cache
  you cannot trust.
  """
  @spec cache_key(String.t(), keyword()) :: String.t()
  def cache_key(prompt, opts \\ []) do
    handles = handles(opts)

    Blake3.hash_hex(
      Enum.join(
        [
          Keyword.get(opts, :model, default_model()),
          Keyword.get(opts, :system) || "",
          prompt,
          handles |> Enum.map(&Ctx.key/1) |> Enum.sort() |> Enum.join(",")
        ],
        <<0>>
      )
    )
  end

  # ── internals ─────────────────────────────────────────────────────────

  @spec ask_item(term(), (term() -> String.t() | {String.t(), keyword()}), keyword()) ::
          {:ok, String.t()} | {:error, error()}
  defp ask_item(item, prompt_fun, opts) do
    {prompt, item_opts} =
      case prompt_fun.(item) do
        {prompt, extra} when is_binary(prompt) and is_list(extra) -> {prompt, extra}
        prompt when is_binary(prompt) -> {prompt, []}
      end

    opts = Keyword.merge(opts, item_opts)
    opts = if match?(%Ctx{}, item), do: Keyword.put_new(opts, :ctx, item), else: opts
    ask(prompt, opts)
  end

  @spec do_ask(String.t(), keyword()) :: {:ok, String.t()} | {:error, error()}
  defp do_ask(prompt, opts) do
    model = Keyword.get(opts, :model, default_model())
    max_tokens = Keyword.get(opts, :max_tokens, @default_max_tokens)
    system = Keyword.get(opts, :system)
    handles = handles(opts)
    key = cache_key(prompt, opts)
    body = assemble(prompt, handles, Keyword.get(opts, :ctx_limit, @default_ctx_limit))

    case cache_probe(Keyword.get(opts, :cache, true), key) do
      {:ok, text} ->
        {:ok, text}

      :miss ->
        request = %{model: model, prompt: body, max_tokens: max_tokens, system: system}
        call(request, key, handles)
    end
  end

  @spec cache_probe(boolean(), String.t()) :: {:ok, String.t()} | :miss
  defp cache_probe(false, _key), do: :miss

  defp cache_probe(true, key) do
    case EventLog.cached(key) do
      nil ->
        :miss

      event ->
        EventLog.append(%{
          kind: :lm_cache_hit,
          cache_key: key,
          payload: %{result_seq: event.seq, model: event.payload["model"]}
        })

        {:ok, Map.get(event.payload, "text", "")}
    end
  end

  @spec call(map(), String.t(), [Ctx.t()]) :: {:ok, String.t()} | {:error, error()}
  defp call(request, key, handles) do
    estimate = div(byte_size(request.prompt), 4) + request.max_tokens
    ctx_ids = Enum.map(handles, &Ctx.key/1)

    case Budget.reserve(estimate) do
      {:error, :budget_exhausted} ->
        EventLog.append(%{
          kind: :lm_budget_refused,
          cache_key: key,
          payload: %{
            model: request.model,
            estimated_tokens: estimate,
            budget: sanitize(Budget.state())
          }
        })

        {:error, :budget_exhausted}

      :ok ->
        started = System.monotonic_time(:millisecond)
        result = backend().complete(request)
        elapsed = System.monotonic_time(:millisecond) - started
        finish(result, request, key, ctx_ids, estimate, elapsed)
    end
  end

  @spec finish(
          {:ok, map()} | {:error, term()},
          map(),
          String.t(),
          [String.t()],
          non_neg_integer(),
          non_neg_integer()
        ) :: {:ok, String.t()} | {:error, error()}
  defp finish({:ok, response}, request, key, ctx_ids, estimate, elapsed) do
    spent = response.tokens_in + response.tokens_out
    Budget.settle(estimate, spent)

    payload = %{
      model: response.model,
      prompt_hash: Blake3.hash_hex(request.prompt),
      ctx_ids: ctx_ids,
      tokens_in: response.tokens_in,
      tokens_out: response.tokens_out,
      latency_ms: elapsed,
      stop_reason: Map.get(response, :stop_reason)
    }

    EventLog.append(%{kind: :lm_ask, cache_key: key, payload: payload})

    # The result row is what a later cache probe finds, and it is written even
    # when this call opted out of READING the cache: `cache: false` means "do
    # not answer me from the log", not "do not record what I learned".
    EventLog.append(%{
      kind: :lm_result,
      cache_key: key,
      payload: payload,
      text: response.text
    })

    {:ok, response.text}
  end

  defp finish({:error, reason}, request, key, ctx_ids, estimate, elapsed) do
    Budget.settle(estimate, div(byte_size(request.prompt), 4))

    EventLog.append(%{
      kind: :lm_error,
      cache_key: key,
      payload: %{
        model: request.model,
        prompt_hash: Blake3.hash_hex(request.prompt),
        ctx_ids: ctx_ids,
        latency_ms: elapsed,
        reason: inspect(reason)
      }
    })

    {:error, reason}
  end

  @spec handles(keyword()) :: [Ctx.t()]
  defp handles(opts) do
    case Keyword.get(opts, :ctx) do
      nil -> []
      %Ctx{} = handle -> [handle]
      list when is_list(list) -> list
    end
  end

  # The one sanctioned crossing: handle contents become prompt text, labelled
  # with the id and window they came from so the sub-model can cite them.
  @spec assemble(String.t(), [Ctx.t()], pos_integer()) :: String.t()
  defp assemble(prompt, [], _limit), do: prompt

  defp assemble(prompt, handles, limit) do
    sections =
      Enum.map_join(handles, "\n\n", fn handle ->
        ~s(<context id="#{Ctx.key(handle)}" lines="#{handle.lines}">\n) <>
          Ctx.read(handle, limit: limit) <> ~s(\n</context>)
      end)

    sections <> "\n\n" <> prompt
  end

  @spec sanitize(map()) :: map()
  defp sanitize(state),
    do: Map.take(state, [:calls, :tokens, :calls_limit, :tokens_limit, :window_ms])
end
