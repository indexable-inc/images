defmodule SymphonyElixirWeb.WorkerSocket do
  @moduledoc """
  The socket runtime workers connect to.

  A worker dials `/worker` and joins the `worker:lobby` channel, advertising the
  address its per-run room-servers are reachable at, its labels, and capacity.
  Identity is the mTLS client-cert CN that the nginx boundary forwards as the
  `x-worker-cn` header; the connection is refused without it. In dev/test, where
  the socket is not behind mTLS, the `worker_id` connect param stands in.
  """

  use Phoenix.Socket

  channel("worker:lobby", SymphonyElixirWeb.WorkerChannel)

  @impl true
  def connect(params, socket, connect_info) do
    case worker_id(params, connect_info) do
      nil ->
        :error

      worker_id ->
        {:ok,
         assign(socket, %{
           worker_id: worker_id,
           address: params["address"],
           labels: parse_labels(params["labels"]),
           capacity: parse_capacity(params["capacity"])
         })}
    end
  end

  @impl true
  def id(socket), do: "worker_socket:#{socket.assigns.worker_id}"

  # The mTLS-verified CN nginx forwards is authoritative; the connect param is
  # the dev/test fallback when the socket is not behind mTLS.
  defp worker_id(params, connect_info) do
    header_cn(connect_info) || empty_to_nil(params["worker_id"])
  end

  defp header_cn(connect_info) do
    connect_info
    |> Map.get(:x_headers, [])
    |> Enum.find_value(fn {name, value} -> if name == "x-worker-cn", do: empty_to_nil(value) end)
  end

  defp parse_labels(nil), do: []

  defp parse_labels(value) when is_binary(value) do
    value |> String.split(",", trim: true) |> Enum.map(&String.trim/1) |> Enum.reject(&(&1 == ""))
  end

  defp parse_capacity(value) when is_binary(value) do
    case Integer.parse(value) do
      {n, _} when n >= 0 -> n
      _ -> 0
    end
  end

  defp parse_capacity(_), do: 0

  defp empty_to_nil(nil), do: nil
  defp empty_to_nil(""), do: nil
  defp empty_to_nil(value) when is_binary(value), do: value
end
