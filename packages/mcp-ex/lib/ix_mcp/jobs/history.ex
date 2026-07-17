defmodule IxMcp.Jobs.History do
  @moduledoc """
  Ordered record of every run: id, intent, session/topic at start time, a code
  preview, and final status. This is what `Jobs.history/1` pages and what the
  `exec` feed groups by session and topic.
  """

  use Agent

  @type entry :: %{
          id: String.t(),
          intent: String.t() | nil,
          session: String.t() | nil,
          topic: String.t() | nil,
          code: String.t(),
          status: IxMcp.Jobs.Job.status(),
          started_at: DateTime.t(),
          elapsed_s: float() | nil
        }

  @spec start_link(term()) :: Agent.on_start()
  def start_link(_opts) do
    Agent.start_link(fn -> [] end, name: __MODULE__)
  end

  @spec record(map()) :: :ok
  def record(entry) do
    Agent.update(__MODULE__, fn entries -> [Map.put(entry, :elapsed_s, nil) | entries] end)
  end

  @spec finished(String.t(), IxMcp.Jobs.Job.status(), float()) :: :ok
  def finished(id, status, elapsed_s) do
    Agent.update(__MODULE__, fn entries ->
      Enum.map(entries, fn
        %{id: ^id} = entry -> %{entry | status: status, elapsed_s: elapsed_s}
        entry -> entry
      end)
    end)
  end

  @doc "Latest `n` runs, newest first."
  @spec list(pos_integer()) :: [entry()]
  def list(n \\ 20) do
    Agent.get(__MODULE__, fn entries -> Enum.take(entries, n) end)
  end
end
