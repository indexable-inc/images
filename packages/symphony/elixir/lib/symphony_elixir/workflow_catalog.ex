defmodule SymphonyElixir.WorkflowCatalog do
  @moduledoc """
  Watches `workflows/*.sym` under the active pack and publishes the latest
  parsed `DSL.AST` for each, hot-reloaded the same way `Catalog` reloads
  YAML DAGs and markdown skills.

  This is the DSL-era ingress index. A producer (cron, a webhook, the
  enqueue UI) resolves an event to a workflow by matching the event against
  each workflow's declared `trigger`, then hands the workflow to the
  ingress to materialize and start an IR run. The catalog owns parsing and
  freshness; it does not start runs.

  Reload semantics match `Catalog`: a new file is parsed and added, changed
  bytes are re-parsed, a deleted file is removed, and a parse error is
  logged while the last good version stays in place. A run snapshots its
  workflow source hash at start, so editing the pack only affects new runs.

  Entries are keyed by file basename and carry the parsed `ast`, the
  declared `trigger` (lifted from the AST for cheap matching), the raw
  `source`, and the `hash` the run records as `RunGraph.source_hash`.

  When a file fails to parse, the last-good entry stays published and the
  located diagnostic (`message`, `line`, `column`, `file`) is recorded
  separately, keyed by basename. `errors/0` and `error/1` expose those so a
  workflows view can show an author exactly where a broken `.sym` failed
  while every other workflow keeps working. A parse error stamps the file's
  basename so the diagnostic names the source even though the parser only
  sees bytes; the error is cleared once the file parses again.
  """

  use GenServer
  require Logger

  alias SymphonyElixir.Config
  alias SymphonyElixir.DSL.Parser

  @table :symphony_workflows
  @errors :symphony_workflow_errors

  defstruct [:workflows_dir, :poll_ms]

  @typedoc "A published workflow: its parsed AST plus the freshness metadata."
  @type entry :: %{
          name: String.t(),
          ast: map(),
          trigger: map() | nil,
          source: String.t(),
          hash: binary()
        }

  @typedoc "A recorded parse failure for one file, keyed by basename."
  @type parse_error :: %{
          name: String.t(),
          message: String.t(),
          line: pos_integer(),
          column: pos_integer(),
          file: String.t()
        }

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc "One workflow by file basename."
  @spec workflow(String.t()) :: {:ok, entry()} | {:error, :not_found}
  def workflow(name) when is_binary(name) do
    case :ets.lookup(@table, name) do
      [{_key, entry}] -> {:ok, entry}
      [] -> {:error, :not_found}
    end
  end

  @doc "Every published workflow."
  @spec workflows() :: [entry()]
  def workflows do
    @table |> :ets.tab2list() |> Enum.map(fn {_name, entry} -> entry end)
  end

  @doc "Workflows whose declared trigger has the given `kind`. The producer's first filter."
  @spec for_trigger_kind(atom()) :: [entry()]
  def for_trigger_kind(kind) when is_atom(kind) do
    Enum.filter(workflows(), fn entry -> match?(%{kind: ^kind}, entry.trigger) end)
  end

  @doc "Every currently broken file's located parse diagnostic."
  @spec errors() :: [parse_error()]
  def errors do
    @errors |> :ets.tab2list() |> Enum.map(fn {_name, err} -> err end)
  end

  @doc "The last parse error for one file basename, if it is currently broken."
  @spec error(String.t()) :: {:ok, parse_error()} | {:error, :not_found}
  def error(name) when is_binary(name) do
    case :ets.lookup(@errors, name) do
      [{_key, err}] -> {:ok, err}
      [] -> {:error, :not_found}
    end
  end

  @impl true
  def init(opts) do
    :ets.new(@table, [:named_table, :public, read_concurrency: true])
    :ets.new(@errors, [:named_table, :public, read_concurrency: true])

    state = %__MODULE__{
      workflows_dir: Keyword.get_lazy(opts, :workflows_dir, fn -> Config.get().workflows_dir end),
      poll_ms: Keyword.get_lazy(opts, :poll_ms, fn -> Config.get().catalog_poll_ms end)
    }

    schedule_scan(0)
    {:ok, state}
  end

  @impl true
  def handle_info(:scan, %__MODULE__{} = state) do
    scan(state.workflows_dir)
    schedule_scan(state.poll_ms)
    {:noreply, state}
  end

  @doc "Scan the workflows directory once, synchronously. Exposed for tests."
  @spec scan(Path.t()) :: :ok
  def scan(dir) do
    files = Path.wildcard(Path.join(dir, "*.sym"))

    seen =
      Enum.reduce(files, MapSet.new(), fn path, acc ->
        name = Path.basename(path, ".sym")
        load_if_changed(name, path)
        MapSet.put(acc, name)
      end)

    remove_missing(seen)
  end

  defp schedule_scan(after_ms), do: Process.send_after(self(), :scan, after_ms)

  defp load_if_changed(name, path) do
    case File.read(path) do
      {:ok, raw} ->
        hash = :crypto.hash(:sha256, raw)

        unless current_hash(name) == hash do
          parse_and_store(name, path, raw, hash)
        end

      {:error, reason} ->
        Logger.warning("WorkflowCatalog failed to read #{path}: #{inspect(reason)}")
    end
  end

  defp parse_and_store(name, path, raw, hash) do
    case Parser.parse(raw, file: Path.basename(path)) do
      {:ok, ast} ->
        entry = %{name: ast.name || name, ast: ast, trigger: ast.trigger, source: raw, hash: hash}
        :ets.insert(@table, {name, entry})
        # A file that parses again clears its prior diagnostic so the
        # workflows view stops showing a stale error.
        :ets.delete(@errors, name)
        Logger.info("WorkflowCatalog loaded workflow=#{name} hash=#{short_hash(hash)}")

      {:error, diag} ->
        # Keep the last-good entry in @table; record the located diagnostic
        # so an author can see where this file broke without losing the
        # workflows that still parse.
        :ets.insert(@errors, {name, error_entry(name, diag)})
        Logger.warning("WorkflowCatalog failed to parse workflow=#{name}: #{inspect(diag)}")
    end
  end

  defp error_entry(name, diag) do
    %{
      name: name,
      message: diag.message,
      line: diag.line,
      column: diag.column,
      file: Map.get(diag, :file) || "#{name}.sym"
    }
  end

  defp current_hash(name) do
    case :ets.lookup(@table, name) do
      [{_key, %{hash: hash}}] -> hash
      _ -> nil
    end
  end

  defp remove_missing(seen) do
    # A deleted file drops both its published entry and any recorded
    # diagnostic; the union of both tables is the set of names a scan might
    # need to retire, since a file can be broken (only in @errors) without a
    # last-good entry in @table.
    (table_names(@table) ++ table_names(@errors))
    |> Enum.uniq()
    |> Enum.each(fn name ->
      unless MapSet.member?(seen, name) do
        :ets.delete(@table, name)
        :ets.delete(@errors, name)
        Logger.info("WorkflowCatalog removed workflow=#{name} (file deleted)")
      end
    end)
  end

  defp table_names(table), do: table |> :ets.tab2list() |> Enum.map(fn {name, _} -> name end)

  defp short_hash(hash), do: hash |> Base.encode16(case: :lower) |> binary_part(0, 8)
end
