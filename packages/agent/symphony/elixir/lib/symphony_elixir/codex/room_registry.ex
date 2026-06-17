defmodule SymphonyElixir.Codex.RoomRegistry do
  @moduledoc """
  Registers per-VM Room servers with a central Room instance.

  Symphony creates short-lived iXVMs and therefore owns the moment a VM
  Room server becomes reachable. The central Room service only stores
  that fact and proxies read paths for the browser UI.
  """

  alias SymphonyElixir.Config

  require Logger

  @spec register(Config.t(), map()) :: :ok
  def register(%Config{room: %{registry_url: nil}}, _backend), do: :ok

  # astlog-ignore: public-def-needs-spec
  def register(%Config{} = config, backend) when is_map(backend) do
    post(config, "/api/backends", backend, "register")
  end

  @spec unregister(Config.t(), String.t()) :: :ok
  def unregister(%Config{room: %{registry_url: nil}}, _id), do: :ok

  # astlog-ignore: public-def-needs-spec
  def unregister(%Config{} = config, id) when is_binary(id) do
    case Req.delete(url(config, "/api/backends/" <> URI.encode(id)),
           headers: headers(config),
           connect_options: [timeout: 5_000],
           receive_timeout: 5_000
         ) do
      {:ok, %{status: status}} when status in 200..299 or status == 404 -> :ok
      {:ok, %{status: status, body: body}} -> warn("unregister", {:status, status, body})
      {:error, reason} -> warn("unregister", reason)
    end
  end

  defp post(%Config{} = config, path, payload, action) do
    case Req.post(url(config, path),
           headers: headers(config),
           json: payload,
           connect_options: [timeout: 5_000],
           receive_timeout: 5_000
         ) do
      {:ok, %{status: status}} when status in 200..299 -> :ok
      {:ok, %{status: status, body: body}} -> warn(action, {:status, status, body})
      {:error, reason} -> warn(action, reason)
    end
  end

  defp url(%Config{room: %{registry_url: registry_url}}, path) do
    String.trim_trailing(registry_url, "/") <> path
  end

  defp headers(%Config{room: %{registry_token: nil}}), do: []
  defp headers(%Config{room: %{registry_token: token}}), do: [{"authorization", "Bearer " <> token}]

  defp warn(action, reason) do
    Logger.warning("RoomRegistry: #{action} failed: #{inspect(reason)}")
    :ok
  end
end
