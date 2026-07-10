defmodule SymphonyElixir.Runtime.Events do
  @moduledoc """
  The one place that owns IR-run PubSub: topic names, the payload shape,
  and the subscribe helpers.

  `Runtime` calls `broadcast/1` after each persisted transition so the
  operator dashboard (`IRRunsLive`) updates without polling. The payload is
  an `IR.View.summary/1` map (string-keyed, JSON-able) so a subscriber can
  refresh an index row from the event alone, and re-read `IR.Store` for the
  detail view when the open run changes.

  Two topics, mirroring the dashboard's two granularities:

  - `"ir_runs"` is the index fan-out: every run transition publishes here so
    the index page can refresh its table.
  - `"ir_run:<run_id>"` is the per-run topic: a detail page subscribes only
    to the run it is showing and ignores the rest of the fleet.

  Both carry the same `{:ir_run_event, run_id, summary}` message, so a
  subscriber pattern-matches one shape regardless of which topic delivered it.
  """

  alias SymphonyElixir.IR.RunGraph
  alias SymphonyElixir.IR.View

  @index_topic "ir_runs"

  @typedoc "The PubSub message every IR-run topic delivers."
  @type event :: {:ir_run_event, run_id :: String.t(), summary :: map()}

  @doc "The index topic every run transition fans out to."
  @spec index_topic() :: String.t()
  def index_topic, do: @index_topic

  @doc "The per-run topic a detail page subscribes to."
  @spec run_topic(String.t()) :: String.t()
  def run_topic(run_id) when is_binary(run_id), do: "ir_run:" <> run_id

  @doc "Subscribe the calling process to the index topic."
  @spec subscribe_index() :: :ok | {:error, term()}
  def subscribe_index, do: Phoenix.PubSub.subscribe(pubsub(), @index_topic)

  @doc "Subscribe the calling process to one run's topic."
  @spec subscribe_run(String.t()) :: :ok | {:error, term()}
  def subscribe_run(run_id) when is_binary(run_id), do: Phoenix.PubSub.subscribe(pubsub(), run_topic(run_id))

  @doc """
  Broadcast a run transition to both the index and the per-run topic. The
  payload is the run's `IR.View.summary/1` so subscribers refresh from the
  event without a store read. Persistence is a separate concern: the caller
  has already written the graph before announcing it.
  """
  @spec broadcast(RunGraph.t()) :: :ok
  def broadcast(%RunGraph{} = graph) do
    message = {:ir_run_event, graph.run_id, View.summary(graph)}
    Phoenix.PubSub.broadcast(pubsub(), @index_topic, message)
    Phoenix.PubSub.broadcast(pubsub(), run_topic(graph.run_id), message)
    :ok
  end

  defp pubsub, do: SymphonyElixir.PubSub
end
