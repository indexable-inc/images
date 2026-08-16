defmodule IxMcp.EventLog.Cas do
  @moduledoc """
  A blake3-addressed blob store for RLM payloads too big to sit in a row.

  Small and deliberately dumb: `put/1` writes `<root>/<ab>/<hex>` and
  returns the id, `get/1` reads it back. Ids are `IxMcp.Blake3.hash_hex/1`,
  the same address `IxMcp.Ctx` and the jj/forge store use, so a blob here
  and the same bytes anywhere else in ix share one name.

  Not the weave store (`IxMcp.Memory`): that one is a separate durable
  service reached through the `weave` binary, and making a sub-model answer
  depend on it would couple every cache hit to an external store's
  availability. This is beside the action-log database, has the same
  lifetime, and is a cache -- a missing blob degrades to a cache miss.
  """

  alias IxMcp.Blake3

  @doc """
  Store `bytes` and return their content id. Idempotent: identical bytes
  land on the same path and are not rewritten.
  """
  @spec put(binary()) :: {:ok, String.t()} | {:error, term()}
  def put(bytes) when is_binary(bytes) do
    id = Blake3.hash_hex(bytes)
    path = path(id)

    if File.exists?(path) do
      {:ok, id}
    else
      with :ok <- File.mkdir_p(Path.dirname(path)),
           tmp = path <> ".#{System.unique_integer([:positive])}.tmp",
           :ok <- File.write(tmp, bytes),
           :ok <- File.rename(tmp, path) do
        {:ok, id}
      end
    end
  end

  @doc """
  Read the blob named `id`.

  `{:error, :missing}` is a normal answer, not a fault: the store is a
  cache, and a caller that cannot find a payload re-derives it.
  """
  @spec get(String.t()) :: {:ok, binary()} | {:error, :missing | term()}
  def get(id) when is_binary(id) do
    case File.read(path(id)) do
      {:ok, bytes} -> {:ok, bytes}
      {:error, :enoent} -> {:error, :missing}
      {:error, reason} -> {:error, reason}
    end
  end

  @doc "Where the blob named `id` lives. Sharded on the first two hex digits."
  @spec path(String.t()) :: Path.t()
  def path(id) when is_binary(id), do: Path.join([root(), String.slice(id, 0, 2), id])

  @doc """
  The store root: `:rlm_cas` app env, else `IX_MCP_RLM_CAS`, else beside
  the action log under `XDG_STATE_HOME`. Same resolution order as the
  database (#3539), so a test can redirect both.
  """
  @spec root() :: Path.t()
  def root do
    Application.get_env(:ix_mcp, :rlm_cas) || System.get_env("IX_MCP_RLM_CAS") ||
      Path.join([state_home(), "ix-mcp-ex", "rlm-cas"])
  end

  @spec state_home() :: Path.t()
  defp state_home do
    System.get_env("XDG_STATE_HOME") || Path.join(System.user_home!(), ".local/state")
  end
end
