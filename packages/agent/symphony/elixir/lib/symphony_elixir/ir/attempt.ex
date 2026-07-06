defmodule SymphonyElixir.IR.Attempt do
  @moduledoc """
  One execution attempt of one `IR.Node`. A node accumulates attempts as
  it is retried or recovered after a crash, so the run record explains
  whether a node was retried, by which engine, at what cost, and how each
  attempt ended.

  `thread_id` is the durable reattach handle for the attempt. For agent
  turns it is the room-server thread/session id; for subrun attempts it is
  the child run id. A node found `:running` after a BEAM restart is
  reconciled from this handle (see `IR.Node` and the runtime recovery
  path).

  `events_ref` points at the streamed event log for the attempt rather
  than inlining it, so the durable run file stays small.
  """

  @enforce_keys [:n, :engine, :state]
  defstruct [
    :n,
    :engine,
    :thread_id,
    :state,
    :started_at,
    :finished_at,
    :outcome,
    :cost,
    :events_ref
  ]

  @typedoc """
  What executed the attempt. `:codex`/`:claude`/`:pi` are engine turns;
  `:exec` is a pack shell script; `:subrun` is a child run. A non-agent
  node has no engine, so its attempt records the executor kind instead of
  a sham `:codex`.
  """
  @type engine :: :codex | :claude | :pi | :exec | :subrun
  @type state :: :running | :succeeded | :failed | :timeout | :cancelled | :stranded

  @typedoc """
  Resolved outcome of a finished attempt. `:stranded` marks an attempt
  whose owning task or BEAM died without reporting a result; the runtime
  cannot assume it had no side effects.
  """
  @type outcome ::
          :ok
          | {:error, term()}
          | :timeout
          | :cancelled
          | :stranded

  @type cost :: %{
          optional(:usd) => float(),
          optional(:tokens_in) => non_neg_integer(),
          optional(:tokens_out) => non_neg_integer(),
          optional(:cache_read) => non_neg_integer(),
          optional(:cache_creation) => non_neg_integer()
        }

  @type t :: %__MODULE__{
          n: pos_integer(),
          engine: engine(),
          thread_id: String.t() | nil,
          state: state(),
          started_at: DateTime.t() | nil,
          finished_at: DateTime.t() | nil,
          outcome: outcome() | nil,
          cost: cost() | nil,
          events_ref: String.t() | nil
        }

  @states [:running, :succeeded, :failed, :timeout, :cancelled, :stranded]
  @engines [:codex, :claude, :pi, :exec, :subrun]

  @doc "The attempt states a persisted attempt may hold. Source of truth for safe decode."
  @spec states() :: [state()]
  def states, do: @states

  @doc "The executor kinds an attempt may record. Source of truth for safe decode."
  @spec engines() :: [engine()]
  def engines, do: @engines

  @spec start(pos_integer(), engine(), String.t() | nil) :: t()
  def start(n, engine, thread_id \\ nil) when is_integer(n) and n > 0 and engine in @engines do
    %__MODULE__{
      n: n,
      engine: engine,
      thread_id: thread_id,
      state: :running,
      started_at: DateTime.utc_now()
    }
  end

  @spec finish(t(), state(), outcome(), cost() | nil) :: t()
  def finish(%__MODULE__{} = attempt, state, outcome, cost \\ nil) do
    %{attempt | state: state, outcome: outcome, cost: cost, finished_at: DateTime.utc_now()}
  end
end
