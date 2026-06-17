defmodule SymphonyElixir.Catalog do
  @moduledoc """
  Watches `skills/*.md` and publishes the latest parsed skills. Polls every
  `catalog_poll_ms` (default 1s) and compares hashes.

  Reload semantics:

  - A new file appears: parsed and added.
  - An existing file's bytes change: re-parsed; old version is replaced.
  - A file is deleted: removed from the catalog.
  - A parse error: kept logged but not crashed; the previously-loaded
    version (if any) stays in place until the bytes parse again.

  Skill resolution is load-bearing for the IR engine path:
  `Runtime.RoomEngineClient` resolves a node's `skill "name"` prompt through
  `Catalog.skill/1`, which expands shared `{{partial:_}}` includes at load
  time. The YAML/DAG stack also watched `dags/`; that surface was deleted in
  the `.sym`/IR cutover (ENG-1828), so this catalog now watches skills only.

  Active runs snapshot the skills they resolve at run start; reloads here
  affect only NEW runs.
  """

  use GenServer
  require Logger

  alias SymphonyElixir.{Config, Skill}

  @table :symphony_catalog

  defstruct [:skills_dir, :poll_ms]

  @type t :: %__MODULE__{skills_dir: Path.t() | nil, poll_ms: non_neg_integer() | nil}

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @spec skill(String.t()) :: {:ok, Skill.t()} | {:error, :not_found}
  def skill(name) when is_binary(name) do
    case :ets.lookup(@table, {:skill, name}) do
      [{_key, skill}] -> {:ok, skill}
      [] -> {:error, :not_found}
    end
  end

  @spec skills() :: [Skill.t()]
  def skills do
    :ets.match_object(@table, {{:skill, :_}, :_})
    |> Enum.map(fn {_key, skill} -> skill end)
  end

  @impl true
  def init(_opts) do
    :ets.new(@table, [:named_table, :public, read_concurrency: true])
    config = Config.get()

    state = %__MODULE__{
      skills_dir: config.skills_dir,
      poll_ms: config.catalog_poll_ms
    }

    schedule_scan(0)
    {:ok, state}
  end

  @impl true
  def handle_info(:scan, %__MODULE__{} = state) do
    scan_dir(state.skills_dir, :skill, &Skill.load/1)
    schedule_scan(state.poll_ms)
    {:noreply, state}
  end

  defp schedule_scan(after_ms) do
    Process.send_after(self(), :scan, after_ms)
  end

  defp scan_dir(dir, :skill, loader) do
    files = Path.wildcard(Path.join(dir, "*.md"))

    seen_names =
      Enum.reduce(files, MapSet.new(), fn path, acc ->
        name = Path.basename(path, Path.extname(path))
        load_if_changed(:skill, name, path, loader)
        MapSet.put(acc, name)
      end)

    remove_missing(:skill, seen_names)
  end

  defp load_if_changed(kind, name, path, loader) do
    case File.read(path) do
      {:ok, raw} ->
        new_hash = :crypto.hash(:sha256, raw)

        case current_hash(kind, name) do
          ^new_hash ->
            :ok

          _ ->
            case loader.(path) do
              {:ok, parsed} ->
                :ets.insert(@table, {{kind, name}, parsed})
                Logger.info("Catalog loaded #{kind}=#{name} hash=#{Base.encode16(new_hash, case: :lower) |> binary_part(0, 8)}")

              {:error, reason} ->
                Logger.warning("Catalog failed to load #{kind}=#{name}: #{inspect(reason)}")
            end
        end

      {:error, reason} ->
        Logger.warning("Catalog failed to read #{path}: #{inspect(reason)}")
    end
  end

  defp current_hash(kind, name) do
    case :ets.lookup(@table, {kind, name}) do
      [{_key, %{body_hash: hash}}] -> hash
      _ -> nil
    end
  end

  defp remove_missing(kind, seen_names) do
    @table
    |> :ets.match_object({{kind, :_}, :_})
    |> Enum.each(fn {{^kind, name} = key, _value} ->
      unless MapSet.member?(seen_names, name) do
        :ets.delete(@table, key)
        Logger.info("Catalog removed #{kind}=#{name} (file deleted)")
      end
    end)
  end
end
