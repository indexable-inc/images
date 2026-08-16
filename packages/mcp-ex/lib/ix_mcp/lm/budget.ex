defmodule IxMcp.LM.Budget do
  @moduledoc """
  A fail-closed spend meter, per workspace, per window.

  Recursive calls fan out, and a fan-out with a bug is a fan-out that spends
  without bound. So `IxMcp.LM` cannot call a model without a reservation
  here, and when the window is used up `ask/2` returns
  `{:error, :budget_exhausted}` -- it never truncates the context, silently
  drops a sub-call, or downgrades the model. A refusal a caller can see and
  raise is worth more than a quiet answer computed from less.

  Two counters per `{workspace, window}`: calls and tokens. Windows are
  fixed buckets (default one hour), so a bucket rolls over rather than
  needing a sweeper.

  Tokens are PRE-CHARGED at an estimate and trued up when the response
  arrives. Charging only afterwards would let a hundred parallel sub-calls
  all read the same under-budget number and pass together, which is exactly
  the shape a recursive fan-out has.

  Limits come from `config :ix_mcp, :lm_budget, calls: _, tokens: _,
  window_ms: _`.

  The ETS table is owned by a lazily started, unsupervised GenServer, like
  `IxMcp.Memory.Semantic`: `IxMcp.LM` must work from a plain IEx with no
  application tree, and the race loser adopts the winner's pid.
  """

  use GenServer

  alias IxMcp.Workspace

  @table :ix_mcp_lm_budget
  @default_calls 50
  @default_tokens 500_000
  @default_window_ms 3_600_000

  @doc """
  Reserve one call and `estimated_tokens` against the current window.

  `{:error, :budget_exhausted}` means the caller must stop, not retry.
  """
  @spec reserve(non_neg_integer()) :: :ok | {:error, :budget_exhausted}
  def reserve(estimated_tokens) when is_integer(estimated_tokens) and estimated_tokens >= 0 do
    limits = limits()
    key = key(limits[:window_ms])
    ensure_table()

    calls = :ets.update_counter(@table, key, {2, 1}, {key, 0, 0})
    tokens = :ets.update_counter(@table, key, {3, estimated_tokens}, {key, 0, 0})

    if calls > limits[:calls] or tokens > limits[:tokens] do
      {:error, :budget_exhausted}
    else
      :ok
    end
  end

  @doc """
  True up a reservation once the real token count is known.

  `estimated` is what `reserve/1` charged; `actual` is what the provider
  reported. The difference is applied, so an over-estimate is refunded.
  """
  @spec settle(non_neg_integer(), non_neg_integer()) :: :ok
  def settle(estimated, actual) when is_integer(estimated) and is_integer(actual) do
    limits = limits()
    key = key(limits[:window_ms])
    ensure_table()
    _ = :ets.update_counter(@table, key, {3, actual - estimated}, {key, 0, 0})
    :ok
  end

  @doc """
  What this workspace has spent in the current window, and what is left.
  """
  @spec state() :: map()
  def state do
    limits = limits()
    key = key(limits[:window_ms])
    ensure_table()

    {calls, tokens} =
      case :ets.lookup(@table, key) do
        [{^key, calls, tokens}] -> {calls, tokens}
        [] -> {0, 0}
      end

    %{
      workspace: elem(key, 0),
      window_ms: limits[:window_ms],
      calls: calls,
      tokens: tokens,
      calls_limit: limits[:calls],
      tokens_limit: limits[:tokens],
      calls_left: max(limits[:calls] - calls, 0),
      tokens_left: max(limits[:tokens] - tokens, 0),
      exhausted?: calls >= limits[:calls] or tokens >= limits[:tokens]
    }
  end

  @doc """
  Forget this workspace's current window. An operator and test affordance;
  it does not raise the limits.
  """
  @spec reset() :: :ok
  def reset do
    ensure_table()
    :ets.delete(@table, key(limits()[:window_ms]))
    :ok
  end

  @doc "The configured limits, with defaults applied."
  @spec limits() :: keyword()
  def limits do
    configured = Application.get_env(:ix_mcp, :lm_budget, [])

    [
      calls: Keyword.get(configured, :calls, @default_calls),
      tokens: Keyword.get(configured, :tokens, @default_tokens),
      window_ms: Keyword.get(configured, :window_ms, @default_window_ms)
    ]
  end

  @doc false
  @impl GenServer
  @spec init(term()) :: {:ok, :ets.table()}
  def init(_arg) do
    {:ok, :ets.new(@table, [:set, :public, :named_table, write_concurrency: true])}
  end

  @spec key(pos_integer()) :: {String.t(), non_neg_integer()}
  defp key(window_ms) do
    {to_string(Workspace.current()), div(System.system_time(:millisecond), window_ms)}
  end

  # Fail closed: if the table cannot be created the meter cannot count, and a
  # meter that cannot count must not let a call through.
  @spec ensure_table() :: :ok
  defp ensure_table do
    if :ets.whereis(@table) == :undefined do
      case GenServer.start(__MODULE__, nil, name: __MODULE__) do
        {:ok, _pid} -> :ok
        {:error, {:already_started, _pid}} -> :ok
        {:error, reason} -> raise "LM budget meter would not start: #{inspect(reason)}"
      end
    end

    :ok
  end
end
