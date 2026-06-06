defmodule SymphonyElixir.CronState do
  @moduledoc """
  Persistent per-workflow fire history for cron-triggered `.sym`
  workflows.

  Owns a single JSON file at `config.cron_state_path` (defaults to
  `runs/cron_state.json`) mapping `name => last_fired_at_iso`. The key is
  the cron producer's choice of identity; it is the workflow basename now
  that `Triggers.Cron` resolves through `WorkflowCatalog`. Writes go
  through this GenServer; reads come straight out of ETS for the hot-path
  tick.

  Survives BEAM restarts. On boot the file is loaded into ETS; missing
  file means nothing has fired yet.

  Public API:

      get_last_fired(name)         -> {:ok, DateTime.t() | nil}
      record_fire(name, at)        -> :ok | {:error, term()}
      list_all()                   -> %{name => DateTime.t()}
      seed_if_unset(name, at)      -> :ok  (idempotent, atomic)

  `seed_if_unset/2` exists so `Triggers.Cron` can mark a newly observed
  workflow as 'last fired = now' without firing it on the boot tick. That
  is the same shape as `systemd Persistent=false` for first observation;
  subsequent missed windows DO trigger one catch-up fire.
  """

  use GenServer
  require Logger

  alias SymphonyElixir.Config

  @table :symphony_cron_state

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @spec get_last_fired(String.t()) :: DateTime.t() | nil
  def get_last_fired(name) when is_binary(name) do
    case :ets.lookup(@table, name) do
      [{^name, %DateTime{} = dt}] -> dt
      _ -> nil
    end
  end

  @spec record_fire(String.t(), DateTime.t()) :: :ok | {:error, term()}
  def record_fire(name, %DateTime{} = at) when is_binary(name) do
    GenServer.call(__MODULE__, {:record, name, at})
  end

  @spec seed_if_unset(String.t(), DateTime.t()) :: :ok
  def seed_if_unset(name, %DateTime{} = at) when is_binary(name) do
    GenServer.call(__MODULE__, {:seed_if_unset, name, at})
  end

  @spec list_all() :: %{String.t() => DateTime.t()}
  def list_all do
    @table
    |> :ets.tab2list()
    |> Map.new(fn {name, dt} -> {name, dt} end)
  end

  @impl true
  def init(_opts) do
    :ets.new(@table, [:named_table, :public, read_concurrency: true])
    path = Config.get().cron_state_path
    load_from_disk(path)
    {:ok, %{path: path}}
  end

  @impl true
  def handle_call({:record, name, %DateTime{} = at}, _from, state) do
    :ets.insert(@table, {name, at})

    case write_to_disk(state.path) do
      :ok -> {:reply, :ok, state}
      {:error, reason} -> {:reply, {:error, reason}, state}
    end
  end

  @impl true
  def handle_call({:seed_if_unset, name, %DateTime{} = at}, _from, state) do
    case :ets.lookup(@table, name) do
      [{^name, _dt}] ->
        {:reply, :ok, state}

      [] ->
        :ets.insert(@table, {name, at})
        _ = write_to_disk(state.path)
        {:reply, :ok, state}
    end
  end

  defp load_from_disk(path) do
    case File.read(path) do
      {:ok, raw} ->
        case Jason.decode(raw) do
          {:ok, %{} = map} ->
            Enum.each(map, fn {name, iso} ->
              case iso |> to_string() |> DateTime.from_iso8601() do
                {:ok, dt, _} -> :ets.insert(@table, {name, dt})
                _ -> Logger.warning("CronState dropped invalid timestamp for #{name}: #{inspect(iso)}")
              end
            end)

          {:ok, other} ->
            Logger.warning("CronState file at #{path} is not a JSON object, ignoring: #{inspect(other)}")

          {:error, reason} ->
            Logger.warning("CronState failed to decode #{path}: #{inspect(reason)}")
        end

      {:error, :enoent} ->
        :ok

      {:error, reason} ->
        Logger.warning("CronState failed to read #{path}: #{inspect(reason)}")
    end
  end

  defp write_to_disk(path) do
    payload =
      @table
      |> :ets.tab2list()
      |> Map.new(fn {name, %DateTime{} = dt} -> {name, DateTime.to_iso8601(dt)} end)

    with {:ok, encoded} <- Jason.encode(payload, pretty: true),
         :ok <- File.mkdir_p(Path.dirname(path)),
         tmp = path <> ".tmp",
         :ok <- File.write(tmp, encoded),
         :ok <- File.rename(tmp, path) do
      :ok
    end
  end
end
