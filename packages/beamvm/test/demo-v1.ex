# Version 1 of the hot-reload demo app: a supervised gen_server that appends
# its compiled-in version and its own pid to $DEMO_OUT every 100 ms. The test
# swaps this module for demo-v2.ex (identical except vsn/0) and requires the
# heartbeat to change version WITHOUT the pid changing.
defmodule Demo.App do
  use Application

  def start(_type, _args) do
    Supervisor.start_link([Demo.Server], strategy: :one_for_one, name: Demo.Supervisor)
  end
end

defmodule Demo.Server do
  use GenServer

  def start_link(_opts) do
    GenServer.start_link(__MODULE__, nil, name: __MODULE__)
  end

  @impl true
  def init(_args) do
    Process.send_after(self(), :beat, 100)
    {:ok, nil}
  end

  @impl true
  def handle_info(:beat, state) do
    out = System.fetch_env!("DEMO_OUT")
    File.write!(out, "vsn=#{vsn()} pid=#{inspect(self())}\n", [:append])
    Process.send_after(self(), :beat, 100)
    {:noreply, state}
  end

  defp vsn, do: 1
end
