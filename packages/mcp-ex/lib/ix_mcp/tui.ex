defmodule IxMcp.Tui do
  @moduledoc """
  Drive a federated TUI resource from a cell -- `Tui.act(uri, send_keys)`
  (aliased in the workspace prelude). Bridges to the federated-resource CLI
  (`ix-resource-cli`), the same binary the Python server shelled out to:
  `act --send-keys=<keys> --peer <url> -- <uri>` drives a peer's live
  terminal resource. When no peer is given, the peers configured in
  `IX_RESOURCE_PEERS` are probed with `get` until one advertises the uri.
  Degrades to a clear error when the CLI is absent.
  """

  @bin "ix-resource-cli"

  @doc ~S{Send literal keystrokes (e.g. `"ls\n"` or `"C-c"`) to the resource at `uri`.}
  @spec act(String.t(), String.t(), String.t() | nil) ::
          {:ok, String.t()} | {:error, String.t()}
  def act(uri, send_keys, peer \\ nil) do
    case System.find_executable(@bin) do
      nil ->
        {:error, "#{@bin} not found on PATH; Tui.act needs the resource CLI"}

      _bin ->
        case peer || find_peer(uri) do
          nil ->
            {:error, "no peer advertises #{uri}; set IX_RESOURCE_PEERS or pass `peer` explicitly"}

          peer_url ->
            run(["act", "--send-keys=#{send_keys}", "--peer", peer_url, "--", uri])
        end
    end
  end

  defp find_peer(uri) do
    "IX_RESOURCE_PEERS"
    |> System.get_env("")
    |> String.split([",", " "], trim: true)
    |> Enum.find(fn peer_url ->
      match?({:ok, _}, run(["get", "--peer", peer_url, "--", uri]))
    end)
  end

  defp run(args) do
    case System.cmd(@bin, args, stderr_to_stdout: true) do
      {out, 0} -> {:ok, String.trim(out)}
      {out, code} -> {:error, "#{@bin} exited #{code}: #{String.slice(out, 0, 400)}"}
    end
  end
end
