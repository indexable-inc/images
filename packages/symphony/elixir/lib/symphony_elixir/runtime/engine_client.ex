defmodule SymphonyElixir.Runtime.EngineClient do
  @moduledoc """
  The seam between the IR runtime and the engine host. The runtime never
  speaks to the room-server directly: it schedules a node by calling
  `run_node/2` through this behaviour, which a later workstream (WS-4)
  implements against the room-server `/api/agent/*` routes. This
  workstream depends only on the behaviour, so its tests use an in-process
  fake and never need a running room-server.

  Two callbacks, matching the two questions the runtime asks:

  - `run_node/2` executes one attempt of a node and returns its terminal
    result. It runs inside a monitored BEAM `Task`; if it raises or the
    BEAM dies, the runtime treats the missing result as a strand (see
    issue #90), so an implementation must return a value rather than
    leaning on the caller to interpret a crash.
  - `status/1` is the restart reattach probe. Given an attempt's
    `thread_id`, it reports whether the engine turn is still alive,
    already finished, or unknown. Recovery uses it to decide whether a
    node found `:running` after a BEAM restart can be harvested or must be
    stranded.

  The `run_opts` map carries the runtime's per-attempt context (the node,
  the attempt number, the run id) so an implementation has what it needs
  without reaching back into runtime state.
  """

  alias SymphonyElixir.IR.Node

  @typedoc "Per-attempt context handed to `run_node/2`."
  @type run_opts :: %{
          required(:run_id) => String.t(),
          required(:attempt) => pos_integer(),
          optional(atom()) => term()
        }

  @typedoc """
  Result of one attempt. `{:ok, output}` succeeds the node; `{:error,
  reason}` fails it. `thread_id` is the engine handle the attempt opened,
  carried so the runtime can record it on the `Attempt` for a later
  reattach probe even when the attempt then fails.
  """
  @type result ::
          {:ok, output :: term(), thread_id :: String.t() | nil}
          | {:error, reason :: term(), thread_id :: String.t() | nil}

  @typedoc """
  Liveness of a previously-started engine turn. `:running` means the turn
  is still in flight and may be reattached; `{:finished, result}` means
  the engine already has a terminal result to harvest; `:unknown` means
  the engine cannot account for the thread (the conservative case, which
  recovery treats as a strand).
  """
  @type turn_status ::
          :running
          | {:finished, {:ok, term()} | {:error, term()}}
          | :unknown

  @callback run_node(Node.t(), run_opts()) :: result()
  @callback status(thread_id :: String.t() | nil) :: turn_status()
end
