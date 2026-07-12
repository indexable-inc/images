defmodule Sample do
  @moduledoc """
  A sample boundary for the emitter tests.
  """

  alias Sample.Native

  defmodule Line do
    @moduledoc """
    A row.
    """

    @enforce_keys [:id, :home]
    defstruct [:id, :home]
    @type t :: %__MODULE__{id: integer(), home: String.t() | nil}
  end

  defmodule SampleFault do
    @moduledoc """
    Boundary failures.
    """

    defstruct [:variant, :message]
    @type t :: %__MODULE__{variant: :missing_store | :invalid, message: String.t()}
  end

  defmodule Cursor do
    @moduledoc """
    A live cursor.
    """

    @typedoc "An opaque handle to a Rust `Cursor`."
    @type t :: reference()

    @doc """
    Open at the start.
    """
    @spec open(String.t()) :: {:ok, t()} | {:error, Sample.SampleFault.t()}
    def open(store) do
      Native.cursor_open(store)
    end

    @doc """
    The current position.
    """
    @spec position(t()) :: integer()
    def position(cursor) do
      Native.cursor_position(cursor)
    end
  end

  @doc """
  Fetch rows.

  Docs become `@doc`s.
  """
  @spec rows(String.t(), integer(), String.t() | nil) :: {:ok, [Sample.Line.t()]} | {:error, Sample.SampleFault.t()}
  def rows(store, limit \\ 10, root \\ nil) do
    Native.rows(store, limit, root)
  end

  @doc """
  Recount everything; long-running, so scheduled dirty.
  """
  @spec recount(String.t()) :: integer()
  def recount(home) do
    Native.recount(home)
  end

  @doc """
  Resolve a label off the scheduler.
  """
  @spec label_of(integer()) :: String.t()
  def label_of(id) do
    ref = make_ref()
    _inflight = Native.label_of(ref, id)
    receive do
      {:unibind, ^ref, {:ok, result}} -> result
    end
  end

  @doc """
  Persist a row.
  """
  @spec store(Sample.Line.t()) :: :ok | {:error, Sample.SampleFault.t()}
  def store(row) do
    ref = make_ref()
    _inflight = Native.store(ref, row)
    receive do
      {:unibind, ^ref, {:ok, _}} -> :ok
      {:unibind, ^ref, {:error, error}} -> {:error, error}
    end
  end

  @doc """
  Every tag, on demand.
  """
  @spec tags(String.t()) :: Enumerable.t()
  def tags(prefix) do
    ref = make_ref()
    handle = Native.tags(ref, prefix)
    unibind_stream(ref, handle)
  end

  @doc """
  Stream rows, verifying the store first.
  """
  @spec scan(String.t()) :: {:ok, Enumerable.t()} | {:error, Sample.SampleFault.t()}
  def scan(store) do
    ref = make_ref()
    case Native.scan(ref, store) do
      {:ok, handle} -> {:ok, unibind_stream(ref, handle)}
      {:error, error} -> {:error, error}
    end
  end

  defp unibind_stream(ref, handle) do
    Stream.resource(
      fn -> handle end,
      fn handle ->
        Native.unibind_demand(handle, 1)

        receive do
          {:unibind_stream, ^ref, {:item, item}} -> {[item], handle}
          {:unibind_stream, ^ref, :done} -> {:halt, handle}
        end
      end,
      fn _handle -> :ok end
    )
  end
end

