defmodule IxMcp.Checkpoint do
  @moduledoc """
  Keeps the workspace's binding and env in an ETS table owned by this process,
  so a crashed or deliberately restarted `IxMcp.Workspace` restores its state
  on init. Because the table never leaves the VM, values need no serialization
  -- pids, refs, and closures all survive a workspace restart intact.

  `save_file/1` / `load_file/1` additionally persist to disk for cross-VM
  session restore, per-name via `:erlang.term_to_binary/1`; names whose values
  cannot be externalized (live pids, ports, refs) are skipped and reported,
  matching the Python session checkpointer's skip-and-report contract.
  """

  use GenServer

  @table __MODULE__

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @impl true
  def init(_) do
    _ = :ets.new(@table, [:named_table, :set, :public, read_concurrency: true])
    {:ok, %{}}
  end

  # The default workspace keeps its historical bare keys (:workspace,
  # :provenance) so a checkpoint written before named workspaces existed
  # restores unchanged; every other workspace rides {:workspace, name} /
  # {:provenance, name} rows in the same table.
  @main "main"

  @spec store(Code.binding(), Macro.Env.t()) :: :ok
  def store(binding, env), do: store(@main, binding, env)

  @spec store(String.t(), Code.binding(), Macro.Env.t()) :: :ok
  def store(workspace, binding, env) do
    :ets.insert(@table, {workspace_key(workspace), binding, env})
    :ok
  end

  @spec fetch() :: {:ok, Code.binding(), Macro.Env.t()} | :empty
  def fetch, do: fetch(@main)

  @spec fetch(String.t()) :: {:ok, Code.binding(), Macro.Env.t()} | :empty
  def fetch(workspace) do
    case :ets.lookup(@table, workspace_key(workspace)) do
      [{_key, binding, env}] -> {:ok, binding, env}
      [] -> :empty
    end
  end

  defp workspace_key(@main), do: :workspace
  defp workspace_key(name), do: {:workspace, name}

  defp provenance_key(@main), do: :provenance
  defp provenance_key(name), do: {:provenance, name}

  @doc """
  Provenance rides its own row (#3967): who bound each variable and each
  module, so `Ix.restart()` comes back still able to say whose value a
  binding is. Kept apart from the binding row so the shape the disk
  checkpoint reads and writes stays exactly what it always was.
  """
  @spec store_provenance(map()) :: :ok
  def store_provenance(provenance), do: store_provenance(@main, provenance)

  @spec store_provenance(String.t(), map()) :: :ok
  def store_provenance(workspace, provenance) do
    :ets.insert(@table, {provenance_key(workspace), provenance})
    :ok
  end

  @spec fetch_provenance() :: %{owners: map(), contested: map(), modules: map()}
  def fetch_provenance, do: fetch_provenance(@main)

  @spec fetch_provenance(String.t()) :: %{owners: map(), contested: map(), modules: map()}
  def fetch_provenance(workspace) do
    empty = %{owners: %{}, contested: %{}, modules: %{}}

    case :ets.lookup(@table, provenance_key(workspace)) do
      [{_key, provenance}] -> Map.merge(empty, provenance)
      [] -> empty
    end
  end

  @spec clear() :: :ok
  def clear, do: clear(@main)

  @spec clear(String.t()) :: :ok
  def clear(workspace) do
    :ets.delete(@table, workspace_key(workspace))
    :ets.delete(@table, provenance_key(workspace))
    :ok
  end

  @doc "Persist the current checkpoint to `path`. Returns names skipped as unserializable."
  @spec save_file(Path.t()) :: {:ok, [atom()]} | {:error, term()}
  def save_file(path) do
    case fetch() do
      :empty ->
        {:error, :empty}

      {:ok, binding, _env} ->
        {kept, skipped} =
          Enum.reduce(binding, {[], []}, fn {name, value}, {kept, skipped} ->
            case try_externalize(value) do
              {:ok, bin} -> {[{name, bin} | kept], skipped}
              :error -> {kept, [name | skipped]}
            end
          end)

        payload = :erlang.term_to_binary({:ix_mcp_checkpoint, 1, Enum.reverse(kept)})

        case File.write(path, payload) do
          :ok -> {:ok, Enum.reverse(skipped)}
          {:error, reason} -> {:error, reason}
        end
    end
  end

  @doc "Load a checkpoint file into the live table (merging over current binding)."
  @spec load_file(Path.t()) :: {:ok, non_neg_integer()} | {:error, term()}
  def load_file(path) do
    with {:ok, bin} <- File.read(path),
         {:ix_mcp_checkpoint, 1, named} <- :erlang.binary_to_term(bin) do
      binding =
        Enum.map(named, fn {name, value_bin} -> {name, :erlang.binary_to_term(value_bin)} end)

      {current, env} =
        case fetch() do
          {:ok, b, e} -> {b, e}
          :empty -> {[], Code.env_for_eval(file: "cell")}
        end

      merged = Keyword.merge(current, binding)
      store(merged, env)
      {:ok, length(binding)}
    else
      {:error, reason} -> {:error, reason}
      _other -> {:error, :bad_checkpoint}
    end
  end

  # A term is externalizable when a round trip through the external term
  # format yields a structurally equal term: pids/ports/refs survive encoding
  # but come back dead, so reject anything containing them outright.
  defp try_externalize(value) do
    if contains_live_ref?(value) do
      :error
    else
      {:ok, :erlang.term_to_binary(value)}
    end
  rescue
    _ -> :error
  end

  defp contains_live_ref?(v) when is_pid(v) or is_port(v) or is_reference(v), do: true
  defp contains_live_ref?(v) when is_function(v), do: not check_fun(v)
  defp contains_live_ref?(v) when is_list(v), do: Enum.any?(v, &contains_live_ref?/1)

  defp contains_live_ref?(v) when is_tuple(v),
    do: v |> Tuple.to_list() |> Enum.any?(&contains_live_ref?/1)

  defp contains_live_ref?(%_struct{} = v),
    do: v |> Map.from_struct() |> Map.values() |> Enum.any?(&contains_live_ref?/1)

  defp contains_live_ref?(v) when is_map(v) do
    Enum.any?(v, fn {k, val} -> contains_live_ref?(k) or contains_live_ref?(val) end)
  end

  defp contains_live_ref?(_), do: false

  # Closures defined in cells reference modules that exist only in this VM;
  # only external (capture) funs of loaded modules survive a file round trip.
  defp check_fun(fun) do
    case Function.info(fun, :type) do
      {:type, :external} -> true
      _ -> false
    end
  end
end
