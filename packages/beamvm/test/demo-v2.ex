# Version 2 of the hot-reload demo app: identical to demo-v1.ex except vsn/0,
# so a hot swap flips the heartbeat version while the server process (and
# therefore its pid) survives untouched.
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

  defp vsn, do: 2
end
